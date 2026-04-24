#![deny(clippy::panic)]
// Following from https://corrode.dev/blog/pitfalls-of-safe-rust/#clippy-can-prevent-many-of-these-issues
// and https://corrode.dev/blog/defensive-programming/#clippy-lints-for-defensive-programming
// Arithmetic
#![deny(arithmetic_overflow)] // Prevent operations that would cause integer overflow
#![deny(clippy::arithmetic_side_effects)] // Detect arithmetic operations with potential side effects
#![deny(clippy::cast_possible_truncation)] // Detect when casting might truncate a value
#![deny(clippy::cast_possible_wrap)] // Detect when casting might cause value to wrap around
#![deny(clippy::cast_precision_loss)] // Detect when casting might lose precision
#![deny(clippy::cast_sign_loss)] // Detect when casting might lose sign information
#![deny(clippy::checked_conversions)] // Suggest using checked conversions between numeric types
#![deny(clippy::integer_division)] // Highlight potential bugs from integer division truncation
#![deny(clippy::unchecked_time_subtraction)] // Ensure duration subtraction won't cause underflow

// Unwraps
#![deny(clippy::expect_used)] // Prevent using .expect() which can cause panics
#![deny(clippy::option_env_unwrap)] // Prevent unwrapping environment variables which might be absent
#![deny(clippy::panicking_unwrap)] // Prevent unwrap on values known to cause panics
#![deny(clippy::unwrap_used)] // Prevent using .unwrap() which can cause panics

// Path handling
#![deny(clippy::join_absolute_paths)] // Prevent issues when joining paths with absolute paths

// Serialization issues
#![deny(clippy::serde_api_misuse)] // Prevent incorrect usage of Serde's serialization/deserialization API

// Unbounded input
#![deny(clippy::uninit_vec)] // Prevent creating uninitialized vectors which is unsafe

// Unsafe code detection
#![deny(clippy::transmute_ptr_to_ref)] // Prevent unsafe transmutation from pointers to references
#![deny(clippy::transmute_undefined_repr)] // Detect transmutes with potentially undefined representations
#![deny(unnecessary_transmutes)] // Prevent unsafe transmutation

// Defensive programming
#![deny(clippy::fallible_impl_from)]
#![deny(clippy::fn_params_excessive_bools)]
#![deny(clippy::must_use_candidate)]
#![deny(clippy::unneeded_field_pattern)]
#![deny(clippy::wildcard_enum_match_arm)]

#[cfg(feature = "python")]
use pyo3::prelude::*;
#[cfg(feature = "python")]
use pyo3_stub_gen::define_stub_info_gatherer;

pub mod convert;
mod decompose;
mod llvm_verify;
pub mod opt;
mod utils;

#[cfg(windows)]
pub const DEFAULT_OPT_LEVEL: u32 = 0;
#[cfg(not(windows))]
pub const DEFAULT_OPT_LEVEL: u32 = 2;

#[cfg(windows)]
pub const DEFAULT_TARGET: &str = "native";
#[cfg(not(windows))]
pub const DEFAULT_TARGET: &str = "aarch64";

mod aux {
    #![allow(clippy::expect_used)]

    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use crate::{
        convert::{
            INIT_QARRAY_FN, LOAD_QUBIT_FN, add_print_call, build_result_global, convert_globals,
            create_reset_call, get_index, get_or_create_function, get_required_num_qubits,
            get_required_num_qubits_strict, get_required_num_results, get_result_vars,
            get_string_label, handle_tuple_or_array_output, parse_gep, record_classical_output,
            replace_rxy_call, replace_rz_call, replace_rzz_call,
        },
        decode_llvm_bytes,
        utils::extract_operands,
    };

    use inkwell::{
        AddressSpace,
        attributes::AttributeLoc,
        basic_block::BasicBlock,
        context::Context,
        module::{Linkage, Module},
        types::{ArrayType, BasicMetadataTypeEnum, BasicTypeEnum, FunctionType},
        values::{
            AnyValue, BasicMetadataValueEnum, BasicValue, BasicValueEnum, CallSiteValue,
            FunctionValue, InstructionOpcode, PointerValue,
        },
    };

    static ALLOWED_QIS_FNS: [&str; 23] = [
        // Native gates
        "__quantum__qis__rxy__body",
        "__quantum__qis__rz__body",
        "__quantum__qis__rzz__body",
        "__quantum__qis__mz__body",
        "__quantum__qis__mz_leaked__body",
        "__quantum__qis__reset__body",
        // mz + reset
        "__quantum__qis__mresetz__body",
        // Synonyms for native gates
        "__quantum__qis__u1q__body", // rxy
        "__quantum__qis__m__body",   // mz
        // Decomposed to native gates
        "__quantum__qis__h__body",
        "__quantum__qis__x__body",
        "__quantum__qis__y__body",
        "__quantum__qis__z__body",
        "__quantum__qis__s__body",
        "__quantum__qis__s__adj",
        "__quantum__qis__t__body",
        "__quantum__qis__t__adj",
        "__quantum__qis__rx__body",
        "__quantum__qis__ry__body",
        "__quantum__qis__cz__body",
        "__quantum__qis__cx__body",
        "__quantum__qis__cnot__body",
        "__quantum__qis__ccx__body",
        // Note: barrier instructions with arbitrary arity are validated separately
    ];

    static BASE_ALLOWED_RT_FNS: [&str; 8] = [
        "__quantum__rt__read_result",
        "__quantum__rt__initialize",
        "__quantum__rt__result_record_output",
        "__quantum__rt__array_record_output",
        "__quantum__rt__tuple_record_output",
        "__quantum__rt__bool_record_output",
        "__quantum__rt__double_record_output",
        "__quantum__rt__int_record_output",
    ];

    #[derive(Clone, Copy, Debug, Default)]
    pub struct CapabilityFlags {
        pub dynamic_qubit_management: bool,
        pub dynamic_result_management: bool,
        pub arrays: bool,
    }

    fn is_capability_gated_rt_function(fn_name: &str) -> bool {
        matches!(
            fn_name,
            "__quantum__rt__qubit_allocate"
                | "__quantum__rt__qubit_release"
                | "__quantum__rt__result_allocate"
                | "__quantum__rt__result_release"
                | "__quantum__rt__qubit_array_allocate"
                | "__quantum__rt__qubit_array_release"
                | "__quantum__rt__result_array_allocate"
                | "__quantum__rt__result_array_release"
                | "__quantum__rt__result_array_record_output"
        )
    }

    fn is_i64_type(type_: BasicMetadataTypeEnum<'_>) -> bool {
        type_.is_int_type() && type_.into_int_type().get_bit_width() == 64
    }

    fn is_ptr_type(type_: BasicMetadataTypeEnum<'_>) -> bool {
        type_.is_pointer_type()
    }

    fn is_void_return(fn_type: FunctionType<'_>) -> bool {
        fn_type.get_return_type().is_none()
    }

    fn is_ptr_return(fn_type: FunctionType<'_>) -> bool {
        fn_type
            .get_return_type()
            .is_some_and(BasicTypeEnum::is_pointer_type)
    }

    fn validate_dynamic_rt_signature(
        fn_name: &str,
        fn_type: FunctionType<'_>,
    ) -> Result<(), String> {
        let params = fn_type.get_param_types();
        let valid = match fn_name {
            "__quantum__rt__qubit_allocate" | "__quantum__rt__result_allocate" => {
                is_ptr_return(fn_type) && params.len() == 1 && is_ptr_type(params[0])
            }
            "__quantum__rt__qubit_release" | "__quantum__rt__result_release" => {
                is_void_return(fn_type) && params.len() == 1 && is_ptr_type(params[0])
            }
            "__quantum__rt__qubit_array_allocate"
            | "__quantum__rt__result_array_allocate"
            | "__quantum__rt__result_array_record_output" => {
                is_void_return(fn_type)
                    && params.len() == 3
                    && is_i64_type(params[0])
                    && is_ptr_type(params[1])
                    && is_ptr_type(params[2])
            }
            "__quantum__rt__qubit_array_release" | "__quantum__rt__result_array_release" => {
                is_void_return(fn_type)
                    && params.len() == 2
                    && is_i64_type(params[0])
                    && is_ptr_type(params[1])
            }
            _ => true,
        };

        if valid {
            Ok(())
        } else {
            Err(format!("Malformed QIR RT function declaration: {fn_name}"))
        }
    }

    pub fn get_capability_flags(module: &Module) -> CapabilityFlags {
        let module_flags = collect_module_flags(module);
        CapabilityFlags {
            dynamic_qubit_management: module_flag_is_enabled(
                &module_flags,
                "dynamic_qubit_management",
            ),
            dynamic_result_management: module_flag_is_enabled(
                &module_flags,
                "dynamic_result_management",
            ),
            arrays: module_flag_is_enabled(&module_flags, "arrays"),
        }
    }

    #[cfg(feature = "wasm")]
    static ALLOWED_QTM_FNS: [&str; 8] = [
        "___get_current_shot",
        "___random_seed",
        "___random_int",
        "___random_float",
        "___random_int_bounded",
        "___random_advance",
        "___get_wasm_context",
        "___barrier",
    ];

    #[cfg(not(feature = "wasm"))]
    static ALLOWED_QTM_FNS: [&str; 7] = [
        "___get_current_shot",
        "___random_seed",
        "___random_int",
        "___random_float",
        "___random_int_bounded",
        "___random_advance",
        "___barrier",
    ];

    #[cfg(not(windows))]
    pub fn validate_module_layout_and_triple(module: &Module) {
        let datalayout = module.get_data_layout();
        let triple = module.get_triple();

        if !datalayout.as_str().is_empty() {
            log::warn!("QIR module has a data layout: {:?}", datalayout.as_str());
        }
        if !triple.as_str().is_empty() {
            log::warn!("QIR module has a target triple: {:?}", triple.as_str());
        }
    }

    #[cfg(windows)]
    pub const fn validate_module_layout_and_triple(_module: &Module) {
        // Best-effort warning path only. Avoid unstable getter APIs on Windows,
        // where these calls have been unreliable in CI; re-checking locally on
        // Windows Arm64 on March 23, 2026 reproduced STATUS_ACCESS_VIOLATION.
    }

    pub fn validate_functions(
        module: &Module,
        entry_fn: FunctionValue,
        _wasm_fns: &BTreeMap<String, u64>,
        errors: &mut Vec<String>,
    ) {
        // Extract required_num_qubits for barrier validation
        let required_num_qubits = get_required_num_qubits(entry_fn);

        for fun in module.get_functions() {
            if fun == entry_fn {
                // Skip the entry function
                continue;
            }
            let fn_name = fun.get_name().to_str().unwrap_or("");
            if fn_name.starts_with("qir_qis.") {
                errors.push(format!(
                    "Input QIR must not define internal helper function: {fn_name}"
                ));
                continue;
            }
            if fn_name.starts_with("__quantum__qis__") {
                // Check for barrier instructions with arbitrary arity (barrier1, barrier2, ...)
                let is_barrier = if fn_name.starts_with("__quantum__qis__barrier")
                    && fn_name.ends_with("__body")
                {
                    parse_barrier_arity(fn_name).is_ok_and(|arity| {
                        // Validate barrier arity doesn't exceed module's required_num_qubits
                        if let Some(max_qubits) = required_num_qubits
                            && let Ok(arity_u32) = u32::try_from(arity)
                            && arity_u32 > max_qubits
                        {
                            errors.push(format!(
                "Barrier arity {arity} exceeds module's required_num_qubits ({max_qubits})"
            ));
                        }
                        true
                    })
                } else {
                    false
                };

                if !is_barrier && !ALLOWED_QIS_FNS.contains(&fn_name) {
                    errors.push(format!("Unsupported QIR QIS function: {fn_name}"));
                }
                continue;
            } else if fn_name.starts_with("__quantum__rt__") {
                if !BASE_ALLOWED_RT_FNS.contains(&fn_name)
                    && !is_capability_gated_rt_function(fn_name)
                {
                    errors.push(format!("Unsupported QIR RT function: {fn_name}"));
                } else if is_capability_gated_rt_function(fn_name)
                    && let Err(err) = validate_dynamic_rt_signature(fn_name, fun.get_type())
                {
                    errors.push(err);
                }
                continue;
            } else if fn_name.starts_with("___") {
                if !ALLOWED_QTM_FNS.contains(&fn_name) {
                    errors.push(format!("Unsupported Qtm QIS function: {fn_name}"));
                }
                continue;
            }

            if fun.count_basic_blocks() > 0 {
                // IR defined functions
                // TODO: allowed only if "ir_functions" is true
                if fn_name == "main" {
                    errors.push("IR defined function cannot be called `main`".to_string());
                }
                // See whether a function returns a pointer type
                if fun
                    .get_type()
                    .get_return_type()
                    .is_some_and(BasicTypeEnum::is_pointer_type)
                {
                    errors.push(format!("Function `{fn_name}` cannot return a pointer type"));
                }
                continue;
            }

            log::debug!(
                "External function `{fn_name}` found, leaving as-is for downstream processing"
            );
        }
    }

    pub fn validate_result_slot_usage(
        module: &Module,
        entry_fn: FunctionValue,
        errors: &mut Vec<String>,
    ) {
        if entry_fn
            .get_string_attribute(AttributeLoc::Function, "required_num_results")
            .is_none()
        {
            return;
        }

        let required_num_results = match get_required_num_results(entry_fn) {
            Ok(required_num_results) => required_num_results,
            Err(err) => {
                errors.push(err);
                return;
            }
        };

        for function in module.get_functions() {
            for bb in function.get_basic_blocks() {
                for instr in bb.get_instructions() {
                    let Ok(call) = CallSiteValue::try_from(instr) else {
                        continue;
                    };
                    let Some(fn_name) = call.get_called_fn_value().and_then(|f| {
                        f.as_global_value()
                            .get_name()
                            .to_str()
                            .ok()
                            .map(ToOwned::to_owned)
                    }) else {
                        continue;
                    };

                    let result_operand_index = match fn_name.as_str() {
                        "__quantum__qis__mz__body"
                        | "__quantum__qis__m__body"
                        | "__quantum__qis__mresetz__body" => 1,
                        "__quantum__rt__read_result" | "__quantum__rt__result_record_output" => 0,
                        _ => continue,
                    };

                    let call_args = match extract_operands(&instr) {
                        Ok(args) => args,
                        Err(err) => {
                            errors.push(format!("Failed to inspect `{fn_name}` call: {err}"));
                            continue;
                        }
                    };
                    let Some(result_arg) = call_args.get(result_operand_index).copied() else {
                        errors.push(format!("Call to `{fn_name}` is missing a result operand"));
                        continue;
                    };

                    let BasicValueEnum::PointerValue(result_ptr) = result_arg else {
                        errors.push(format!(
                            "Call to `{fn_name}` has a non-pointer result operand"
                        ));
                        continue;
                    };

                    let result_idx = match get_index(result_ptr) {
                        Ok(idx) => idx,
                        Err(err) => {
                            errors.push(format!(
                                "Failed to inspect result operand for `{fn_name}`: {err}"
                            ));
                            continue;
                        }
                    };
                    if let Err(err) = checked_result_index(result_idx, required_num_results) {
                        errors.push(err);
                    }
                }
            }
        }
    }

    pub fn validate_module_flags(module: &Module, errors: &mut Vec<String>) {
        let module_flags = collect_module_flags(module);
        validate_exact_module_flag(
            &module_flags,
            "qir_major_version",
            &["i32 1", "i32 2"],
            errors,
        );
        validate_exact_module_flag(&module_flags, "qir_minor_version", &["i32 0"], errors);
        validate_exact_module_flag(
            &module_flags,
            "dynamic_qubit_management",
            &["i1 false", "i1 true"],
            errors,
        );
        validate_exact_module_flag(
            &module_flags,
            "dynamic_result_management",
            &["i1 false", "i1 true"],
            errors,
        );
        validate_optional_module_flag(&module_flags, "arrays", &["i1 false", "i1 true"], errors);
    }

    pub struct ModuleFlags {
        values: BTreeMap<String, Vec<String>>,
        malformed: BTreeSet<String>,
    }

    impl ModuleFlags {
        pub fn get(&self, flag_name: &str) -> Option<&[String]> {
            self.values.get(flag_name).map(Vec::as_slice)
        }

        fn is_malformed(&self, flag_name: &str) -> bool {
            self.malformed.contains(flag_name)
        }
    }

    pub fn collect_module_flags(module: &Module) -> ModuleFlags {
        let mut values = BTreeMap::<String, Vec<String>>::new();
        let mut malformed = BTreeSet::new();

        for entry in module.get_global_metadata("llvm.module.flags") {
            let Some(node_values) = entry.get_node_values() else {
                continue;
            };
            let flag_name = extract_module_flag_name(&node_values);

            if node_values.len() != 3 {
                if let Some(flag_name) = flag_name {
                    malformed.insert(flag_name);
                }
                continue;
            }

            let Some(flag_name) = flag_name else {
                continue;
            };

            let Some(flag_value) = format_module_flag_value(node_values[2]) else {
                malformed.insert(flag_name);
                continue;
            };

            values.entry(flag_name).or_default().push(flag_value);
        }

        ModuleFlags { values, malformed }
    }

    fn extract_module_flag_name(values: &[BasicMetadataValueEnum]) -> Option<String> {
        values
            .get(1)?
            .into_metadata_value()
            .get_string_value()
            .and_then(decode_llvm_bytes)
            .map(str::to_owned)
    }

    fn format_module_flag_value(value: BasicMetadataValueEnum) -> Option<String> {
        match value {
            BasicMetadataValueEnum::IntValue(value) => {
                let bit_width = value.get_type().get_bit_width();
                let raw_value = value.get_zero_extended_constant()?;
                if bit_width == 1 {
                    Some(format!(
                        "i1 {}",
                        if raw_value == 0 { "false" } else { "true" }
                    ))
                } else {
                    Some(format!("i{bit_width} {raw_value}"))
                }
            }
            BasicMetadataValueEnum::MetadataValue(value) => value
                .get_string_value()
                .and_then(decode_llvm_bytes)
                .map(|string| format!("!\"{string}\"")),
            BasicMetadataValueEnum::ArrayValue(_)
            | BasicMetadataValueEnum::FloatValue(_)
            | BasicMetadataValueEnum::PointerValue(_)
            | BasicMetadataValueEnum::StructValue(_)
            | BasicMetadataValueEnum::VectorValue(_)
            | BasicMetadataValueEnum::ScalableVectorValue(_) => None,
        }
    }

    fn module_flag_is_enabled(module_flags: &ModuleFlags, flag_name: &str) -> bool {
        module_flags
            .get(flag_name)
            .is_some_and(|values| values.iter().any(|value| value == "i1 true"))
    }

    fn validate_optional_module_flag(
        module_flags: &ModuleFlags,
        flag_name: &str,
        expected_values: &[&str],
        errors: &mut Vec<String>,
    ) {
        let Some(actual_values) = module_flags.get(flag_name) else {
            if module_flags.is_malformed(flag_name) {
                errors.push(format!("Missing or unsupported module flag: {flag_name}"));
            }
            return;
        };

        if actual_values
            .iter()
            .any(|actual| expected_values.contains(&actual.as_str()))
        {
            return;
        }

        let expected = if expected_values.len() == 1 {
            expected_values[0].to_string()
        } else {
            format!("one of {}", expected_values.join(", "))
        };
        errors.push(format!("Unsupported {flag_name}: expected {expected}"));
    }
    fn validate_exact_module_flag(
        module_flags: &ModuleFlags,
        flag_name: &str,
        expected_values: &[&str],
        errors: &mut Vec<String>,
    ) {
        let Some(actual_values) = module_flags.get(flag_name) else {
            if module_flags.is_malformed(flag_name) {
                errors.push(format!("Missing or unsupported module flag: {flag_name}"));
                return;
            }
            errors.push(format!("Missing required module flag: {flag_name}"));
            return;
        };

        if actual_values
            .iter()
            .any(|actual| expected_values.contains(&actual.as_str()))
        {
            return;
        }

        let expected = if expected_values.len() == 1 {
            expected_values[0].to_string()
        } else {
            format!("one of {}", expected_values.join(", "))
        };
        errors.push(format!("Unsupported {flag_name}: expected {expected}"));
    }

    fn get_fixed_pointer_array_len(
        array_ptr: PointerValue<'_>,
        opname: &str,
    ) -> Result<u64, String> {
        let array_backing_error =
            || format!("{opname} requires a fixed-size backing array allocated as [N x ptr]");
        let Some(instr) = array_ptr.as_instruction_value() else {
            return Err(array_backing_error());
        };
        let opcode = instr.get_opcode();

        if opcode == InstructionOpcode::Alloca {
            let allocated_type = instr
                .get_allocated_type()
                .map_err(|_| array_backing_error())?;
            let BasicTypeEnum::ArrayType(array_type) = allocated_type else {
                return Err(array_backing_error());
            };
            if !matches!(array_type.get_element_type(), BasicTypeEnum::PointerType(_)) {
                return Err(array_backing_error());
            }
            return Ok(u64::from(array_type.len()));
        }

        if opcode == InstructionOpcode::BitCast || opcode == InstructionOpcode::AddrSpaceCast {
            return instr
                .get_operand(0)
                .and_then(inkwell::values::Operand::value)
                .map(BasicValueEnum::into_pointer_value)
                .ok_or_else(array_backing_error)
                .and_then(|backing_ptr| get_fixed_pointer_array_len(backing_ptr, opname));
        }

        if opcode == InstructionOpcode::GetElementPtr {
            for operand_idx in 1..instr.get_num_operands() {
                let Some(operand) = instr.get_operand(operand_idx) else {
                    return Err(array_backing_error());
                };
                let inkwell::values::Operand::Value(value) = operand else {
                    return Err(array_backing_error());
                };
                let idx = value
                    .into_int_value()
                    .get_zero_extended_constant()
                    .ok_or_else(array_backing_error)?;
                if idx != 0 {
                    return Err(array_backing_error());
                }
            }

            return instr
                .get_operand(0)
                .and_then(inkwell::values::Operand::value)
                .map(BasicValueEnum::into_pointer_value)
                .ok_or_else(array_backing_error)
                .and_then(|backing_ptr| get_fixed_pointer_array_len(backing_ptr, opname));
        }

        Err(array_backing_error())
    }

    pub fn validate_dynamic_array_allocation_backing(module: &Module, errors: &mut Vec<String>) {
        for fun in module.get_functions() {
            for bb in fun.get_basic_blocks() {
                for instr in bb.get_instructions() {
                    let Ok(call) = CallSiteValue::try_from(instr) else {
                        continue;
                    };
                    let Some(callee) = call.get_called_fn_value() else {
                        continue;
                    };
                    let callee_global = callee.as_global_value();
                    let callee_name = callee_global.get_name();
                    let Some(fn_name) = callee_name.to_str().ok() else {
                        continue;
                    };
                    if !matches!(
                        fn_name,
                        "__quantum__rt__qubit_array_allocate"
                            | "__quantum__rt__result_array_allocate"
                            | "__quantum__rt__result_array_record_output"
                    ) {
                        continue;
                    }

                    let call_args: Vec<BasicValueEnum> = match extract_operands(&instr) {
                        Ok(args) => args,
                        Err(err) => {
                            errors.push(format!("Failed to inspect {fn_name} operands: {err}"));
                            continue;
                        }
                    };
                    let requested_len = match extract_const_len(call_args[0], fn_name) {
                        Ok(len) => len,
                        Err(err) => {
                            errors.push(err);
                            continue;
                        }
                    };
                    let backing_len = match get_fixed_pointer_array_len(
                        call_args[1].into_pointer_value(),
                        fn_name,
                    ) {
                        Ok(len) => len,
                        Err(err) => {
                            errors.push(err);
                            continue;
                        }
                    };
                    if requested_len != backing_len {
                        errors.push(format!(
                            "{fn_name} requires a fixed-size backing array whose requested length {requested_len} does not match backing array length {backing_len}"
                        ));
                    }
                    if fn_name == "__quantum__rt__result_array_record_output"
                        && requested_len > i32::MAX as u64
                    {
                        errors.push(format!(
                            "{fn_name} requires an array length that fits in i32 for RESULT_ARRAY output"
                        ));
                    }
                }
            }
        }
    }

    pub fn validate_capability_usage(
        module: &Module,
        flags: CapabilityFlags,
        errors: &mut Vec<String>,
    ) {
        for fun in module.get_functions() {
            for bb in fun.get_basic_blocks() {
                for instr in bb.get_instructions() {
                    let Ok(call) = CallSiteValue::try_from(instr) else {
                        continue;
                    };
                    let Some(callee) = call.get_called_fn_value() else {
                        continue;
                    };
                    let callee_global = callee.as_global_value();
                    let callee_name = callee_global.get_name();
                    let Some(fn_name) = callee_name.to_str().ok() else {
                        continue;
                    };

                    match fn_name {
                        "__quantum__rt__qubit_array_allocate"
                        | "__quantum__rt__qubit_array_release"
                            if !flags.arrays || !flags.dynamic_qubit_management =>
                        {
                            errors.push(format!(
                                "{fn_name} requires both `arrays=true` and `dynamic_qubit_management=true`"
                            ));
                        }
                        "__quantum__rt__result_array_allocate"
                        | "__quantum__rt__result_array_release"
                        | "__quantum__rt__result_array_record_output"
                            if !flags.arrays || !flags.dynamic_result_management =>
                        {
                            errors.push(format!(
                                "{fn_name} requires both `arrays=true` and `dynamic_result_management=true`"
                            ));
                        }
                        "__quantum__rt__qubit_allocate" | "__quantum__rt__qubit_release"
                            if !flags.dynamic_qubit_management =>
                        {
                            errors.push(format!(
                                "{fn_name} requires `dynamic_qubit_management=true`"
                            ));
                        }
                        "__quantum__rt__result_allocate" | "__quantum__rt__result_release"
                            if !flags.dynamic_result_management =>
                        {
                            errors.push(format!(
                                "{fn_name} requires `dynamic_result_management=true`"
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub fn validate_dynamic_result_allocation_placement(
        module: &Module,
        entry_fn: FunctionValue,
        errors: &mut Vec<String>,
    ) {
        for fun in module.get_functions() {
            let allowed_block = if fun == entry_fn {
                fun.get_first_basic_block()
            } else {
                None
            };

            for bb in fun.get_basic_blocks() {
                for instr in bb.get_instructions() {
                    let Ok(call) = CallSiteValue::try_from(instr) else {
                        continue;
                    };
                    let Some(callee) = call.get_called_fn_value() else {
                        continue;
                    };
                    let callee_global = callee.as_global_value();
                    let callee_name = callee_global.get_name();
                    let Some(fn_name) = callee_name.to_str().ok() else {
                        continue;
                    };
                    if matches!(
                        fn_name,
                        "__quantum__rt__result_allocate" | "__quantum__rt__result_array_allocate"
                    ) && Some(bb) != allowed_block
                    {
                        errors.push(format!(
                            "{fn_name} is only supported in the entry block because dynamic result slots are lowered to stack storage"
                        ));
                    }
                }
            }
        }
    }

    // SAFETY: `ProcessCallArgs` is created and consumed synchronously within a single
    // `process_call_instruction` invocation. The raw pointers below point at
    // stack-owned state from `process_entry_function` that outlives the handler
    // call, and handlers never persist those pointers beyond the call.
    struct ProcessCallArgs<'ctx> {
        ctx: &'ctx Context,
        module: *const Module<'ctx>,
        instr: inkwell::values::InstructionValue<'ctx>,
        fn_name: String,
        // Reserved for downstream passthrough compatibility.
        #[allow(dead_code)]
        wasm_fns: *const BTreeMap<String, u64>,
        qubit_array: Option<PointerValue<'ctx>>,
        qubit_array_type: Option<ArrayType<'ctx>>,
        capability_flags: CapabilityFlags,
        global_mapping: *mut HashMap<String, inkwell::values::GlobalValue<'ctx>>,
        result_ssa: *mut Vec<Option<(BasicValueEnum<'ctx>, Option<BasicValueEnum<'ctx>>)>>,
    }

    /// Primary translation loop over the entry function for translation to QIS.
    pub fn process_entry_function<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
        entry_fn: FunctionValue<'ctx>,
        wasm_fns: &BTreeMap<String, u64>,
        qubit_array: Option<PointerValue<'ctx>>,
        capability_flags: CapabilityFlags,
    ) -> Result<(), String> {
        let mut global_mapping = convert_globals(ctx, module)?;

        if global_mapping.is_empty() {
            log::warn!("No globals found in QIR module");
        }
        let mut result_ssa = if capability_flags.dynamic_result_management {
            Vec::new()
        } else {
            get_result_vars(entry_fn)?
        };
        let qubit_array_type = if capability_flags.dynamic_qubit_management {
            None
        } else {
            Some(
                ctx.i64_type()
                    .array_type(get_required_num_qubits_strict(entry_fn)?),
            )
        };

        for bb in entry_fn.get_basic_blocks() {
            // Snapshot instructions before rewriting calls. Some rewrite paths
            // erase/replace instructions, which can invalidate in-place iterators.
            let instructions: Vec<_> = bb.get_instructions().collect();
            for instr in instructions {
                let Ok(call) = CallSiteValue::try_from(instr) else {
                    continue;
                };
                if let Some(fn_name) = call.get_called_fn_value().and_then(|f| {
                    f.as_global_value()
                        .get_name()
                        .to_str()
                        .ok()
                        .map(ToOwned::to_owned)
                }) {
                    let args = ProcessCallArgs {
                        ctx,
                        module,
                        instr,
                        fn_name,
                        wasm_fns: std::ptr::from_ref(wasm_fns),
                        qubit_array,
                        qubit_array_type,
                        capability_flags,
                        global_mapping: &raw mut global_mapping,
                        result_ssa: &raw mut result_ssa,
                    };
                    process_call_instruction(args)?;
                }
            }
        }

        Ok(())
    }

    fn process_call_instruction(mut args: ProcessCallArgs<'_>) -> Result<(), String> {
        let call = CallSiteValue::try_from(args.instr)
            .map_err(|()| "Instruction is not a call site".to_string())?;
        match args.fn_name.as_str() {
            name if name.starts_with("__quantum__qis__") => handle_qis_call(&args),
            name if name.starts_with("__quantum__rt__") => handle_rt_call(&mut args),
            name if name.starts_with("___") => handle_qtm_call(&args),
            _ => {
                if let Some(f) = call.get_called_fn_value() {
                    // IR defined function calls
                    if f.count_basic_blocks() > 0 {
                        // INIT_QARRAY_FN is a frequently invoked helper for array initialization;
                        // skipping debug logs for it avoids excessive log noise while preserving
                        // useful debug information for other IR-defined functions.
                        if args.fn_name != INIT_QARRAY_FN {
                            log::debug!("IR defined function `{}`: {}", args.fn_name, f.get_type());
                        }
                        if args.fn_name == "main" {
                            return Err("IR defined function cannot be called `main`".to_string());
                        }
                        return Ok(());
                    }

                    // Check if this is a GPU function (has cudaq-fnid attribute)
                    if f.get_string_attribute(AttributeLoc::Function, "cudaq-fnid")
                        .is_some()
                    {
                        log::debug!(
                            "GPU function `{}` found, leaving as-is for downstream processing",
                            args.fn_name
                        );
                        return Ok(());
                    }

                    // Check if this is a WASM function (has wasm attribute)
                    if f.get_string_attribute(AttributeLoc::Function, "wasm")
                        .is_some()
                    {
                        log::debug!(
                            "WASM function `{}` found, leaving as-is for downstream processing",
                            args.fn_name
                        );
                        return Ok(());
                    }

                    log::error!("Unknown external function: {}", args.fn_name);
                    return Err(format!("Unsupported function: {}", args.fn_name));
                }

                log::error!("Unsupported function: {}", args.fn_name);
                Err(format!("Unsupported function: {}", args.fn_name))
            }
        }
    }

    fn handle_qis_call(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        match args.fn_name.as_str() {
            "__quantum__qis__rxy__body" => {
                replace_rxy_call(
                    args.ctx,
                    module_ref(args),
                    args.instr,
                    args.capability_flags.dynamic_qubit_management,
                )?;
            }
            "__quantum__qis__rz__body" => {
                replace_rz_call(
                    args.ctx,
                    module_ref(args),
                    args.instr,
                    args.capability_flags.dynamic_qubit_management,
                )?;
            }
            "__quantum__qis__rzz__body" => {
                replace_rzz_call(
                    args.ctx,
                    module_ref(args),
                    args.instr,
                    args.capability_flags.dynamic_qubit_management,
                )?;
            }
            "__quantum__qis__u1q__body" => {
                log::info!(
                    "`__quantum__qis__u1q__body` used, synonym for `__quantum__qis__rxy__body`"
                );
                replace_rxy_call(
                    args.ctx,
                    module_ref(args),
                    args.instr,
                    args.capability_flags.dynamic_qubit_management,
                )?;
            }
            "__quantum__qis__mz__body"
            | "__quantum__qis__m__body"
            | "__quantum__qis__mresetz__body" => {
                handle_mz_call(
                    args.ctx,
                    args.module.cast::<()>(),
                    &args.instr,
                    args.fn_name.as_str(),
                    args.capability_flags,
                    args.qubit_array,
                    args.qubit_array_type,
                    args.result_ssa.cast::<()>(),
                )?;
            }
            "__quantum__qis__mz_leaked__body" => {
                handle_mz_leaked_call(args)?;
            }
            "__quantum__qis__reset__body" => {
                handle_reset_call(args)?;
            }
            name if name.starts_with("__quantum__qis__barrier") && name.ends_with("__body") => {
                handle_barrier_call(args)?;
            }
            _ => {
                // Under LLVM 21, decomposition functions may remain as IR-defined calls
                // rather than being fully inlined at this stage. Allow these calls to
                // pass through; their bodies are lowered by process_ir_defined_q_fns.
                let is_ir_defined = module_ref(args)
                    .get_function(args.fn_name.as_str())
                    .is_some_and(|f| f.count_basic_blocks() > 0);
                if !is_ir_defined {
                    return Err(format!("Unsupported QIR QIS function: {}", args.fn_name));
                }
            }
        }
        Ok(())
    }

    fn handle_rt_call(args: &mut ProcessCallArgs<'_>) -> Result<(), String> {
        match args.fn_name.as_str() {
            "__quantum__rt__initialize" => {
                args.instr.erase_from_basic_block();
            }
            "__quantum__rt__qubit_allocate" => {
                lower_dynamic_qubit_allocate(args)?;
            }
            "__quantum__rt__qubit_release" => {
                lower_dynamic_qubit_release(args)?;
            }
            "__quantum__rt__qubit_array_allocate" => {
                lower_dynamic_qubit_array_allocate(args)?;
            }
            "__quantum__rt__qubit_array_release" => {
                lower_dynamic_qubit_array_release(args)?;
            }
            "__quantum__rt__result_allocate" => {
                lower_dynamic_result_allocate(args)?;
            }
            "__quantum__rt__result_release" => {
                lower_dynamic_result_release(args)?;
            }
            "__quantum__rt__result_array_allocate" => {
                lower_dynamic_result_array_allocate(args)?;
            }
            "__quantum__rt__result_array_release" => {
                lower_dynamic_result_array_release(args)?;
            }
            "__quantum__rt__result_array_record_output" => {
                lower_dynamic_result_array_record_output(args)?;
            }
            "__quantum__rt__read_result" | "__quantum__rt__result_record_output" => {
                handle_read_result_call(
                    args.ctx,
                    args.module.cast::<()>(),
                    &args.instr,
                    args.fn_name.as_str(),
                    args.capability_flags,
                    args.global_mapping.cast::<()>(),
                    args.result_ssa.cast::<()>(),
                )?;
            }
            "__quantum__rt__tuple_record_output" | "__quantum__rt__array_record_output" => {
                let fn_name = args.fn_name.clone();
                handle_tuple_or_array_output(
                    args.ctx,
                    module_ref(args),
                    args.instr,
                    unsafe { &mut *args.global_mapping },
                    fn_name.as_str(),
                )?;
            }
            "__quantum__rt__bool_record_output"
            | "__quantum__rt__int_record_output"
            | "__quantum__rt__double_record_output" => {
                handle_classical_record_output(args)?;
            }
            _ => return Err(format!("Unsupported QIR RT function: {}", args.fn_name)),
        }
        Ok(())
    }

    fn handle_qtm_call(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        match args.fn_name.as_str() {
            "___get_current_shot" => {
                handle_get_current_shot(args)?;
            }
            "___random_seed" => {
                handle_random_seed(args)?;
            }
            "___random_int" => {
                handle_random_int(args)?;
            }
            "___random_float" => {
                handle_random_float(args)?;
            }
            "___random_int_bounded" => {
                handle_random_int_bounded(args)?;
            }
            "___random_advance" => {
                handle_random_advance(args)?;
            }
            "___get_wasm_context" => {
                // External context calls are left as-is for downstream processing
                log::debug!("___get_wasm_context found, leaving as-is for downstream processing");
            }
            _ => {
                // Ignore already converted Qtm QIS functions
                log::trace!("Ignoring Qtm QIS function: {}", args.fn_name);
            }
        }
        Ok(())
    }

    fn get_qubit_handle<'ctx>(
        ctx: &'ctx Context,
        capability_flags: CapabilityFlags,
        qubit_array: Option<PointerValue<'ctx>>,
        qubit_array_type: Option<ArrayType<'ctx>>,
        builder: &inkwell::builder::Builder<'ctx>,
        qubit_ptr: PointerValue<'ctx>,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if capability_flags.dynamic_qubit_management {
            return builder
                .build_ptr_to_int(qubit_ptr, ctx.i64_type(), "qbit")
                .map(BasicValueEnum::from)
                .map_err(|e| format!("Failed to convert qubit pointer to handle: {e}"));
        }

        let qubit_array = qubit_array.ok_or("Missing static qubit array for qubit lookup")?;
        let qubit_array_type =
            qubit_array_type.ok_or("Missing static qubit array type for qubit lookup")?;
        let i64_type = ctx.i64_type();
        let index = get_index(qubit_ptr)?;
        let index_val = i64_type.const_int(index, false);
        let elem_ptr = unsafe {
            builder.build_gep(
                qubit_array_type,
                qubit_array,
                &[i64_type.const_zero(), index_val],
                "",
            )
        }
        .map_err(|e| format!("Failed to build GEP for qubit handle: {e}"))?;
        builder
            .build_load(i64_type, elem_ptr, "qbit")
            .map_err(|e| format!("Failed to build load for qubit handle: {e}"))
    }

    const fn module_ref<'ctx>(args: &ProcessCallArgs<'ctx>) -> &'ctx Module<'ctx> {
        // SAFETY: `args.module` points to the borrowed module passed into
        // `process_entry_function`, which outlives all handler calls.
        unsafe { &*args.module }
    }

    fn get_or_create_qalloc_fail_global<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> inkwell::values::GlobalValue<'ctx> {
        let panic_msg = ctx.const_string(b".EXIT:INT:No more qubits available to allocate.", false);
        let panic_arr_ty = panic_msg.get_type();
        module.get_global("e_qalloc_fail").unwrap_or_else(|| {
            let global = module.add_global(panic_arr_ty, None, "e_qalloc_fail");
            global.set_initializer(&panic_msg);
            global.set_linkage(Linkage::Private);
            global.set_constant(true);
            global
        })
    }

    struct DynamicQubitAllocatePaths<'ctx> {
        fail: BasicBlock<'ctx>,
        fail_store: BasicBlock<'ctx>,
        fail_panic: BasicBlock<'ctx>,
        success: BasicBlock<'ctx>,
        store_ok: BasicBlock<'ctx>,
        ret_ok: BasicBlock<'ctx>,
    }

    fn build_dynamic_qubit_allocate_fail_path<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
        builder: &inkwell::builder::Builder<'ctx>,
        ptr_type: inkwell::types::PointerType<'ctx>,
        out_err: PointerValue<'ctx>,
        paths: &DynamicQubitAllocatePaths<'ctx>,
    ) {
        builder.position_at_end(paths.fail);
        let out_err_is_null =
            build_ptr_is_null(ctx, builder, out_err, "out_err_int").expect("out_err null check");
        builder
            .build_conditional_branch(out_err_is_null, paths.fail_panic, paths.fail_store)
            .expect("branch fail handler");

        builder.position_at_end(paths.fail_store);
        let _ = builder.build_store(out_err, ctx.bool_type().const_all_ones());
        builder
            .build_return(Some(&ptr_type.const_zero()))
            .expect("return null qubit");

        builder.position_at_end(paths.fail_panic);
        let err_global = get_or_create_qalloc_fail_global(ctx, module);
        let err_ty = err_global
            .get_initializer()
            .expect("panic global initializer")
            .into_array_value()
            .get_type();
        let err_gep = unsafe {
            builder.build_gep(
                err_ty,
                err_global.as_pointer_value(),
                &[ctx.i64_type().const_zero(), ctx.i64_type().const_zero()],
                "err_gep",
            )
        }
        .expect("panic msg gep");
        let panic_fn = get_or_create_function(
            module,
            "panic",
            ctx.void_type()
                .fn_type(&[ctx.i32_type().into(), ptr_type.into()], false),
        );
        let _ = builder.build_call(
            panic_fn,
            &[ctx.i32_type().const_int(1001, false).into(), err_gep.into()],
            "",
        );
        builder.build_unreachable().expect("unreachable");
    }

    fn build_dynamic_qubit_allocate_success_path<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        ptr_type: inkwell::types::PointerType<'ctx>,
        out_err: PointerValue<'ctx>,
        qid: inkwell::values::IntValue<'ctx>,
        paths: &DynamicQubitAllocatePaths<'ctx>,
    ) {
        builder.position_at_end(paths.success);
        let out_err_is_null =
            build_ptr_is_null(ctx, builder, out_err, "out_err_int_ok").expect("out_err null check");
        builder
            .build_conditional_branch(out_err_is_null, paths.ret_ok, paths.store_ok)
            .expect("branch success handler");

        builder.position_at_end(paths.store_ok);
        let _ = builder.build_store(out_err, ctx.bool_type().const_zero());
        builder
            .build_unconditional_branch(paths.ret_ok)
            .expect("jump ret");

        builder.position_at_end(paths.ret_ok);
        let ptr_val = builder
            .build_int_to_ptr(qid, ptr_type, "qubit_ptr")
            .expect("int to ptr");
        builder
            .build_return(Some(&ptr_val))
            .expect("return qubit ptr");
    }

    struct DynamicQubitArrayAllocateBlocks<'ctx> {
        loop_header: BasicBlock<'ctx>,
        loop_body: BasicBlock<'ctx>,
        loop_exit: BasicBlock<'ctx>,
        continue_alloc: BasicBlock<'ctx>,
        maybe_rollback: BasicBlock<'ctx>,
        rollback: BasicBlock<'ctx>,
        rollback_done: BasicBlock<'ctx>,
    }

    struct PointerArrayReleaseHelperArgs<'ctx> {
        function: FunctionValue<'ctx>,
        len: inkwell::values::IntValue<'ctx>,
        array_ptr: PointerValue<'ctx>,
        release_fn: FunctionValue<'ctx>,
        ptr_elem_type: inkwell::types::PointerType<'ctx>,
        gep_error: &'static str,
        load_error: &'static str,
        release_error: &'static str,
    }

    fn create_dynamic_qubit_array_allocate_blocks<'ctx>(
        ctx: &'ctx Context,
        function: FunctionValue<'ctx>,
    ) -> DynamicQubitArrayAllocateBlocks<'ctx> {
        DynamicQubitArrayAllocateBlocks {
            loop_header: ctx.append_basic_block(function, "loop_header"),
            loop_body: ctx.append_basic_block(function, "loop_body"),
            loop_exit: ctx.append_basic_block(function, "loop_exit"),
            continue_alloc: ctx.append_basic_block(function, "continue_alloc"),
            maybe_rollback: ctx.append_basic_block(function, "maybe_rollback"),
            rollback: ctx.append_basic_block(function, "rollback"),
            rollback_done: ctx.append_basic_block(function, "rollback_done"),
        }
    }

    fn build_dynamic_qubit_array_allocate_rollback<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        idx: inkwell::values::IntValue<'ctx>,
        array_ptr: PointerValue<'ctx>,
        release_array_fn: FunctionValue<'ctx>,
        rollback: BasicBlock<'ctx>,
        rollback_done: BasicBlock<'ctx>,
    ) {
        builder.position_at_end(rollback);
        let rollback_len = builder
            .build_int_add(idx, ctx.i64_type().const_int(1, false), "rollback_len")
            .expect("rollback len");
        let _ = builder
            .build_call(
                release_array_fn,
                &[rollback_len.into(), array_ptr.into()],
                "",
            )
            .expect("rollback release");
        builder
            .build_unconditional_branch(rollback_done)
            .expect("jump rollback_done");
    }

    fn get_bool_cl_array_type(ctx: &Context) -> inkwell::types::StructType<'_> {
        ctx.struct_type(
            &[
                ctx.i32_type().into(),
                ctx.i32_type().into(),
                ctx.ptr_type(AddressSpace::default()).into(),
                ctx.ptr_type(AddressSpace::default()).into(),
            ],
            true,
        )
    }

    fn build_dynamic_result_array_values<'ctx>(
        ctx: &'ctx Context,
        function: FunctionValue<'ctx>,
        builder: &inkwell::builder::Builder<'ctx>,
        len: inkwell::values::IntValue<'ctx>,
        array_ptr: PointerValue<'ctx>,
        read_fn: FunctionValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let bool_arr = builder
            .build_array_alloca(ctx.bool_type(), len, "result_arr_data")
            .map_err(|e| format!("Failed to allocate result array data: {e}"))?;
        build_dynamic_array_loop(ctx, function, builder, len, |builder, idx| {
            let elem_ptr = unsafe { builder.build_gep(ptr_type, array_ptr, &[idx], "elem_ptr") }
                .map_err(|e| format!("Failed to build result record array GEP: {e}"))?;
            let result_ptr = builder
                .build_load(ptr_type, elem_ptr, "result_ptr")
                .map_err(|e| format!("Failed to load result pointer: {e}"))?;
            let bool_call = builder
                .build_call(read_fn, &[result_ptr.into()], "result_bool")
                .map_err(|e| format!("Failed to read result array element: {e}"))?;
            let bool_val = match bool_call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err("Dynamic result read helper did not return a bool".to_string());
                }
            };
            let out_ptr =
                unsafe { builder.build_gep(ctx.bool_type(), bool_arr, &[idx], "result_bool_ptr") }
                    .map_err(|e| format!("Failed to index result bool array: {e}"))?;
            let _ = builder
                .build_store(out_ptr, bool_val)
                .map_err(|e| format!("Failed to store result bool array element: {e}"))?;
            Ok(())
        })?;
        Ok(bool_arr)
    }

    fn build_dynamic_result_array_descriptor<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        len: inkwell::values::IntValue<'ctx>,
        bool_arr: PointerValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, String> {
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let array_desc_type = get_bool_cl_array_type(ctx);
        let array_desc = builder
            .build_alloca(array_desc_type, "result_arr_desc")
            .map_err(|e| format!("Failed to allocate result array descriptor: {e}"))?;
        let x_ptr = builder
            .build_struct_gep(array_desc_type, array_desc, 0, "result_arr_x")
            .map_err(|e| format!("Failed to build result array length GEP: {e}"))?;
        let y_ptr = builder
            .build_struct_gep(array_desc_type, array_desc, 1, "result_arr_y")
            .map_err(|e| format!("Failed to build result array rank GEP: {e}"))?;
        let data_ptr = builder
            .build_struct_gep(array_desc_type, array_desc, 2, "result_arr_data_ptr")
            .map_err(|e| format!("Failed to build result array data GEP: {e}"))?;
        let mask_ptr = builder
            .build_struct_gep(array_desc_type, array_desc, 3, "result_arr_mask_ptr")
            .map_err(|e| format!("Failed to build result array mask GEP: {e}"))?;
        let mask_zero = builder
            .build_alloca(ctx.i32_type(), "result_arr_mask")
            .map_err(|e| format!("Failed to allocate result array mask: {e}"))?;
        let _ = builder
            .build_store(mask_zero, ctx.i32_type().const_zero())
            .map_err(|e| format!("Failed to initialize result array mask: {e}"))?;
        let len_i32 = builder
            .build_int_truncate(len, ctx.i32_type(), "result_arr_len")
            .map_err(|e| format!("Failed to truncate result array length: {e}"))?;
        let _ = builder
            .build_store(x_ptr, len_i32)
            .map_err(|e| format!("Failed to store result array length: {e}"))?;
        let _ = builder
            .build_store(y_ptr, ctx.i32_type().const_int(1, false))
            .map_err(|e| format!("Failed to store result array rank: {e}"))?;
        let data_as_ptr = builder
            .build_bit_cast(bool_arr, ptr_type, "result_arr_data_cast")
            .map_err(|e| format!("Failed to cast result array data pointer: {e}"))?;
        let _ = builder
            .build_store(data_ptr, data_as_ptr)
            .map_err(|e| format!("Failed to store result array data pointer: {e}"))?;
        let mask_as_ptr = builder
            .build_bit_cast(mask_zero, ptr_type, "result_arr_mask_cast")
            .map_err(|e| format!("Failed to cast result array mask pointer: {e}"))?;
        let _ = builder
            .build_store(mask_ptr, mask_as_ptr)
            .map_err(|e| format!("Failed to store result array mask pointer: {e}"))?;
        Ok(array_desc)
    }

    struct DynamicResultSlotPtrs<'ctx> {
        state: PointerValue<'ctx>,
        cached: PointerValue<'ctx>,
        future: PointerValue<'ctx>,
    }

    fn get_dynamic_result_slot_ptrs<'ctx>(
        builder: &inkwell::builder::Builder<'ctx>,
        slot_type: inkwell::types::StructType<'ctx>,
        result_ptr: PointerValue<'ctx>,
    ) -> DynamicResultSlotPtrs<'ctx> {
        DynamicResultSlotPtrs {
            state: builder
                .build_struct_gep(slot_type, result_ptr, 0, "state")
                .expect("state gep"),
            cached: builder
                .build_struct_gep(slot_type, result_ptr, 1, "cached")
                .expect("cached gep"),
            future: builder
                .build_struct_gep(slot_type, result_ptr, 2, "future")
                .expect("future gep"),
        }
    }

    fn build_dynamic_result_pending_return<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
        builder: &inkwell::builder::Builder<'ctx>,
        future_ptr: PointerValue<'ctx>,
        cached_ptr: PointerValue<'ctx>,
        state_ptr: PointerValue<'ctx>,
    ) {
        let future = builder
            .build_load(ctx.i64_type(), future_ptr, "future")
            .expect("load future");
        let read_fn = get_or_create_function(
            module,
            "___read_future_bool",
            ctx.bool_type().fn_type(&[ctx.i64_type().into()], false),
        );
        let dec_fn = get_or_create_function(
            module,
            "___dec_future_refcount",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );
        let bool_call = builder
            .build_call(read_fn, &[future.into()], "bool")
            .expect("read future");
        let bool_val = match bool_call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv,
            inkwell::values::ValueKind::Instruction(_) => unreachable!(),
        };
        let _ = builder.build_call(dec_fn, &[future.into()], "");
        let _ = builder.build_store(cached_ptr, bool_val);
        let _ = builder.build_store(state_ptr, ctx.i8_type().const_int(2, false));
        builder
            .build_return(Some(&bool_val.into_int_value()))
            .expect("ret pending");
    }

    fn call_basic_value<'ctx>(
        builder: &inkwell::builder::Builder<'ctx>,
        callee: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
        build_error: &str,
        value_error: &str,
    ) -> Result<BasicValueEnum<'ctx>, String> {
        let call = builder
            .build_call(callee, args, name)
            .map_err(|e| format!("{build_error}: {e}"))?;
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => Ok(bv),
            inkwell::values::ValueKind::Instruction(_) => Err(value_error.to_string()),
        }
    }

    fn clear_dynamic_result_slot<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        slot_ptrs: &DynamicResultSlotPtrs<'ctx>,
    ) {
        let _ = builder.build_store(slot_ptrs.state, ctx.i8_type().const_zero());
        let _ = builder.build_store(slot_ptrs.cached, ctx.bool_type().const_zero());
        let _ = builder.build_store(slot_ptrs.future, ctx.i64_type().const_zero());
    }

    fn set_dynamic_result_pending<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        slot_ptrs: &DynamicResultSlotPtrs<'ctx>,
        future: inkwell::values::IntValue<'ctx>,
    ) {
        let _ = builder.build_store(slot_ptrs.state, ctx.i8_type().const_int(1, false));
        let _ = builder.build_store(slot_ptrs.future, future);
    }

    fn read_result_bool<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
        builder: &inkwell::builder::Builder<'ctx>,
        result_ptr: PointerValue<'ctx>,
        capability_flags: CapabilityFlags,
        result_ssa: &mut [Option<(BasicValueEnum<'ctx>, Option<BasicValueEnum<'ctx>>)>],
    ) -> Result<BasicValueEnum<'ctx>, String> {
        if capability_flags.dynamic_result_management {
            let read_func = ensure_dynamic_result_read(ctx, module);
            return call_basic_value(
                builder,
                read_func,
                &[result_ptr.into()],
                "bool",
                "Failed to build call for dynamic result read",
                "Failed to get basic value from dynamic result read call",
            );
        }

        let result_idx = get_index(result_ptr)?;
        let result_idx_usize = usize::try_from(result_idx)
            .map_err(|e| format!("Failed to convert result index to usize: {e}"))?;
        let meas_handle = result_ssa[result_idx_usize]
            .ok_or_else(|| "Expected measurement handle".to_string())?;
        result_ssa[result_idx_usize]
            .and_then(|v| v.1)
            .and_then(|val: BasicValueEnum<'_>| val.as_instruction_value())
            .map_or_else(
                || {
                    let read_func = get_or_create_function(
                        module,
                        "___read_future_bool",
                        ctx.bool_type().fn_type(&[ctx.i64_type().into()], false),
                    );
                    let bool_val = call_basic_value(
                        builder,
                        read_func,
                        &[meas_handle.0.into()],
                        "bool",
                        "Failed to build call for read_future_bool",
                        "Failed to get basic value from read_future_bool call",
                    )?;
                    let dec_func = get_or_create_function(
                        module,
                        "___dec_future_refcount",
                        ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
                    );
                    let _ = builder
                        .build_call(dec_func, &[meas_handle.0.into()], "")
                        .map_err(|e| {
                            format!("Failed to build call for dec_future_refcount: {e}")
                        })?;
                    result_ssa[result_idx_usize] = Some((meas_handle.0, Some(bool_val)));
                    Ok(bool_val)
                },
                |val: inkwell::values::InstructionValue<'_>| {
                    val.as_any_value_enum()
                        .try_into()
                        .map_err(|()| "Expected BasicValueEnum".to_string())
                },
            )
    }

    fn get_or_create_result_output_global<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
        global_mapping: &mut HashMap<String, inkwell::values::GlobalValue<'ctx>>,
        capability_flags: CapabilityFlags,
        result_ptr: PointerValue<'ctx>,
        gep: BasicValueEnum<'ctx>,
    ) -> Result<inkwell::values::GlobalValue<'ctx>, String> {
        if let Ok(old_global) = parse_gep(gep) {
            return global_mapping
                .get(old_global.as_str())
                .copied()
                .ok_or_else(|| format!("Output global `{old_global}` not found in mapping"));
        }

        let fallback_label = if capability_flags.dynamic_result_management {
            "result_dynamic".to_string()
        } else {
            format!("result_{}", get_index(result_ptr)?)
        };
        let (new_const, new_name) =
            build_result_global(ctx, &fallback_label, &fallback_label, "RESULT", None)?;
        let new_global = module.add_global(new_const.get_type(), None, &new_name);
        new_global.set_initializer(&new_const);
        new_global.set_linkage(inkwell::module::Linkage::Private);
        new_global.set_constant(true);
        global_mapping.insert(fallback_label, new_global);
        Ok(new_global)
    }

    fn get_dynamic_result_slot_type(ctx: &Context) -> inkwell::types::StructType<'_> {
        ctx.struct_type(
            &[
                ctx.i8_type().into(),   // state: 0=false, 1=pending future, 2=cached bool
                ctx.bool_type().into(), // cached bool
                ctx.i64_type().into(),  // future handle
            ],
            false,
        )
    }

    fn build_ptr_is_null<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        ptr: PointerValue<'ctx>,
        name: &str,
    ) -> Result<inkwell::values::IntValue<'ctx>, String> {
        let ptr_as_int = builder
            .build_ptr_to_int(ptr, ctx.i64_type(), name)
            .map_err(|e| format!("Failed to convert pointer to int: {e}"))?;
        builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                ptr_as_int,
                ctx.i64_type().const_zero(),
                "is_null",
            )
            .map_err(|e| format!("Failed to compare pointer with null: {e}"))
    }

    fn ensure_dynamic_qubit_allocate<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.qubit_allocate") {
            return existing;
        }

        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let fn_type = ptr_type.fn_type(&[ptr_type.into()], false);
        let function =
            module.add_function("qir_qis.qubit_allocate", fn_type, Some(Linkage::Private));
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        let paths = DynamicQubitAllocatePaths {
            fail: ctx.append_basic_block(function, "fail"),
            fail_store: ctx.append_basic_block(function, "fail_store"),
            fail_panic: ctx.append_basic_block(function, "fail_panic"),
            success: ctx.append_basic_block(function, "success"),
            store_ok: ctx.append_basic_block(function, "store_ok"),
            ret_ok: ctx.append_basic_block(function, "ret_ok"),
        };
        builder.position_at_end(entry);

        let out_err = function
            .get_first_param()
            .expect("qubit allocate helper has out_err")
            .into_pointer_value();
        let qalloc_fn =
            get_or_create_function(module, "___qalloc", ctx.i64_type().fn_type(&[], false));
        let call_result = builder
            .build_call(qalloc_fn, &[], "qalloc")
            .expect("qalloc call");
        let qid = match call_result.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => unreachable!(),
        };
        let is_fail = builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                qid,
                ctx.i64_type().const_int(u64::MAX, false),
                "is_fail",
            )
            .expect("compare fail");
        builder
            .build_conditional_branch(is_fail, paths.fail, paths.success)
            .expect("branch fail");
        build_dynamic_qubit_allocate_fail_path(ctx, module, &builder, ptr_type, out_err, &paths);
        build_dynamic_qubit_allocate_success_path(ctx, &builder, ptr_type, out_err, qid, &paths);
        function
    }

    fn ensure_dynamic_qubit_release<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.qubit_release") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let fn_type = ctx.void_type().fn_type(&[ptr_type.into()], false);
        let function =
            module.add_function("qir_qis.qubit_release", fn_type, Some(Linkage::Private));
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        let ret = ctx.append_basic_block(function, "ret");
        let body = ctx.append_basic_block(function, "body");
        builder.position_at_end(entry);
        let qubit_ptr = function
            .get_first_param()
            .expect("qubit release param")
            .into_pointer_value();
        let is_null = build_ptr_is_null(ctx, &builder, qubit_ptr, "qubit_int").expect("null check");
        builder
            .build_conditional_branch(is_null, ret, body)
            .expect("branch");
        builder.position_at_end(body);
        let q_handle = builder
            .build_ptr_to_int(qubit_ptr, ctx.i64_type(), "qbit")
            .expect("ptr to int");
        let qfree_fn = get_or_create_function(
            module,
            "___qfree",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );
        let _ = builder.build_call(qfree_fn, &[q_handle.into()], "");
        builder.build_unconditional_branch(ret).expect("jump ret");
        builder.position_at_end(ret);
        builder.build_return(None).expect("return");
        function
    }

    fn build_dynamic_array_loop<'ctx, F>(
        ctx: &'ctx Context,
        function: FunctionValue<'ctx>,
        builder: &inkwell::builder::Builder<'ctx>,
        trip_count: inkwell::values::IntValue<'ctx>,
        mut body_builder: F,
    ) -> Result<(), String>
    where
        F: FnMut(
            &inkwell::builder::Builder<'ctx>,
            inkwell::values::IntValue<'ctx>,
        ) -> Result<(), String>,
    {
        let entry_block = builder
            .get_insert_block()
            .ok_or("Missing loop entry block")?;
        let loop_header = ctx.append_basic_block(function, "loop_header");
        let loop_body = ctx.append_basic_block(function, "loop_body");
        let loop_exit = ctx.append_basic_block(function, "loop_exit");
        builder
            .build_unconditional_branch(loop_header)
            .map_err(|e| format!("Failed to branch to loop header: {e}"))?;
        builder.position_at_end(loop_header);
        let idx_phi = builder
            .build_phi(ctx.i64_type(), "idx")
            .map_err(|e| format!("Failed to create loop phi: {e}"))?;
        idx_phi.add_incoming(&[(&ctx.i64_type().const_zero(), entry_block)]);
        let idx = idx_phi.as_basic_value().into_int_value();
        let cond = builder
            .build_int_compare(inkwell::IntPredicate::ULT, idx, trip_count, "loop_cond")
            .map_err(|e| format!("Failed to build loop condition: {e}"))?;
        builder
            .build_conditional_branch(cond, loop_body, loop_exit)
            .map_err(|e| format!("Failed to build loop branch: {e}"))?;
        builder.position_at_end(loop_body);
        body_builder(builder, idx)?;
        let next = builder
            .build_int_add(idx, ctx.i64_type().const_int(1, false), "next_idx")
            .map_err(|e| format!("Failed to increment loop index: {e}"))?;
        builder
            .build_unconditional_branch(loop_header)
            .map_err(|e| format!("Failed to jump to loop header: {e}"))?;
        idx_phi.add_incoming(&[(&next, loop_body)]);
        builder.position_at_end(loop_exit);
        Ok(())
    }

    fn build_pointer_array_release_helper<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        args: &PointerArrayReleaseHelperArgs<'ctx>,
    ) -> Result<(), String> {
        build_dynamic_array_loop(ctx, args.function, builder, args.len, |builder, idx| {
            let elem_ptr = unsafe {
                builder.build_gep(args.ptr_elem_type, args.array_ptr, &[idx], "elem_ptr")
            }
            .map_err(|e| format!("{}: {e}", args.gep_error))?;
            let value_ptr = builder
                .build_load(args.ptr_elem_type, elem_ptr, "value_ptr")
                .map_err(|e| format!("{}: {e}", args.load_error))?;
            let _ = builder
                .build_call(args.release_fn, &[value_ptr.into()], "")
                .map_err(|e| format!("{}: {e}", args.release_error))?;
            Ok(())
        })
    }

    fn ensure_dynamic_qubit_array_allocate<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.qubit_array_allocate") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let function = module.add_function(
            "qir_qis.qubit_array_allocate",
            ctx.void_type().fn_type(
                &[ctx.i64_type().into(), ptr_type.into(), ptr_type.into()],
                false,
            ),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        builder.position_at_end(entry);
        let len = function.get_nth_param(0).expect("len").into_int_value();
        let array_ptr = function
            .get_nth_param(1)
            .expect("array")
            .into_pointer_value();
        let out_err = function
            .get_nth_param(2)
            .expect("out_err")
            .into_pointer_value();
        let alloc_fn = ensure_dynamic_qubit_allocate(ctx, module);
        let out_err_success_fn = ensure_out_err_success(ctx, module);
        let _ = builder
            .build_call(out_err_success_fn, &[out_err.into()], "")
            .expect("initialize out_err");
        let release_array_fn = ensure_dynamic_qubit_array_release(ctx, module);
        let entry_block = entry;
        let blocks = create_dynamic_qubit_array_allocate_blocks(ctx, function);

        builder
            .build_unconditional_branch(blocks.loop_header)
            .expect("jump loop_header");
        builder.position_at_end(blocks.loop_header);
        let idx_phi = builder.build_phi(ctx.i64_type(), "idx").expect("idx phi");
        idx_phi.add_incoming(&[(&ctx.i64_type().const_zero(), entry_block)]);
        let idx = idx_phi.as_basic_value().into_int_value();
        let cond = builder
            .build_int_compare(inkwell::IntPredicate::ULT, idx, len, "loop_cond")
            .expect("loop cond");
        builder
            .build_conditional_branch(cond, blocks.loop_body, blocks.loop_exit)
            .expect("loop branch");

        builder.position_at_end(blocks.loop_body);
        let elem_ptr = unsafe { builder.build_gep(ptr_type, array_ptr, &[idx], "elem_ptr") }
            .expect("elem gep");
        let slot = builder
            .build_call(alloc_fn, &[out_err.into()], "qubit_slot")
            .expect("alloc qubit");
        let slot = match slot.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv,
            inkwell::values::ValueKind::Instruction(_) => {
                unreachable!("Dynamic qubit allocation did not return a pointer");
            }
        };
        let _ = builder.build_store(elem_ptr, slot).expect("store qubit");
        let out_err_is_null =
            build_ptr_is_null(ctx, &builder, out_err, "out_err_int").expect("out_err null check");
        builder
            .build_conditional_branch(
                out_err_is_null,
                blocks.continue_alloc,
                blocks.maybe_rollback,
            )
            .expect("branch maybe_rollback");

        builder.position_at_end(blocks.maybe_rollback);
        let failed = builder
            .build_load(ctx.bool_type(), out_err, "alloc_failed")
            .expect("load out_err")
            .into_int_value();
        builder
            .build_conditional_branch(failed, blocks.rollback, blocks.continue_alloc)
            .expect("branch rollback");
        build_dynamic_qubit_array_allocate_rollback(
            ctx,
            &builder,
            idx,
            array_ptr,
            release_array_fn,
            blocks.rollback,
            blocks.rollback_done,
        );

        builder.position_at_end(blocks.continue_alloc);
        let next = builder
            .build_int_add(idx, ctx.i64_type().const_int(1, false), "next_idx")
            .expect("next idx");
        builder
            .build_unconditional_branch(blocks.loop_header)
            .expect("jump loop_header");
        idx_phi.add_incoming(&[(&next, blocks.continue_alloc)]);

        builder.position_at_end(blocks.loop_exit);
        builder.build_return(None).expect("return");

        builder.position_at_end(blocks.rollback_done);
        builder.build_return(None).expect("return");
        function
    }

    fn ensure_dynamic_qubit_array_release<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.qubit_array_release") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let function = module.add_function(
            "qir_qis.qubit_array_release",
            ctx.void_type()
                .fn_type(&[ctx.i64_type().into(), ptr_type.into()], false),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        builder.position_at_end(entry);
        let len = function.get_nth_param(0).expect("len").into_int_value();
        let array_ptr = function
            .get_nth_param(1)
            .expect("array")
            .into_pointer_value();
        let release_fn = ensure_dynamic_qubit_release(ctx, module);
        build_pointer_array_release_helper(
            ctx,
            &builder,
            &PointerArrayReleaseHelperArgs {
                function,
                len,
                array_ptr,
                release_fn,
                ptr_elem_type: ptr_type,
                gep_error: "Failed to build qubit array GEP",
                load_error: "Failed to load dynamic qubit pointer",
                release_error: "Failed to release dynamic qubit",
            },
        )
        .expect("build dynamic qubit release loop");
        builder.build_return(None).expect("return");
        function
    }

    fn replace_call_with_value<'ctx>(
        instr: inkwell::values::InstructionValue<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Result<(), String> {
        let instruction_val = value
            .as_instruction_value()
            .ok_or("Expected replacement value to be instruction-backed")?;
        instr.replace_all_uses_with(&instruction_val);
        instr.erase_from_basic_block();
        Ok(())
    }

    fn initialize_dynamic_result_slot<'ctx>(
        ctx: &'ctx Context,
        builder: &inkwell::builder::Builder<'ctx>,
        slot_ptr: PointerValue<'ctx>,
    ) {
        let slot_type = get_dynamic_result_slot_type(ctx);
        let slot_ptrs = get_dynamic_result_slot_ptrs(builder, slot_type, slot_ptr);
        clear_dynamic_result_slot(ctx, builder, &slot_ptrs);
    }

    fn ensure_out_err_success<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.out_err_success") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let function = module.add_function(
            "qir_qis.out_err_success",
            ctx.void_type().fn_type(&[ptr_type.into()], false),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        let ret = ctx.append_basic_block(function, "ret");
        let set_ok = ctx.append_basic_block(function, "set_ok");
        builder.position_at_end(entry);
        let out_err = function
            .get_first_param()
            .expect("out_err")
            .into_pointer_value();
        let is_null = build_ptr_is_null(ctx, &builder, out_err, "out_err_int").expect("null");
        builder
            .build_conditional_branch(is_null, ret, set_ok)
            .expect("branch");
        builder.position_at_end(set_ok);
        let _ = builder.build_store(out_err, ctx.bool_type().const_zero());
        builder.build_unconditional_branch(ret).expect("jump");
        builder.position_at_end(ret);
        builder.build_return(None).expect("ret");
        function
    }

    fn extract_const_len(value: BasicValueEnum<'_>, opname: &str) -> Result<u64, String> {
        value
            .into_int_value()
            .get_zero_extended_constant()
            .ok_or_else(|| format!("{opname} currently requires a constant array length"))
    }

    fn lower_void_helper_call<'ctx>(
        ctx: &'ctx Context,
        instr: inkwell::values::InstructionValue<'ctx>,
        helper: FunctionValue<'ctx>,
        call_args: &[BasicValueEnum<'ctx>],
        error_context: &str,
    ) -> Result<(), String> {
        let builder = ctx.create_builder();
        builder.position_before(&instr);
        let metadata_args: Vec<BasicMetadataValueEnum<'ctx>> =
            call_args.iter().copied().map(Into::into).collect();
        let _ = builder
            .build_call(helper, &metadata_args, "")
            .map_err(|e| format!("{error_context}: {e}"))?;
        instr.erase_from_basic_block();
        Ok(())
    }

    fn lower_dynamic_qubit_allocate(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let builder = args.ctx.create_builder();
        builder.position_before(&args.instr);
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let helper = ensure_dynamic_qubit_allocate(args.ctx, module_ref(args));
        let value = call_basic_value(
            &builder,
            helper,
            &[call_args[0].into()],
            "dyn_q",
            "Failed to lower dynamic qubit allocation",
            "Dynamic qubit allocate helper did not return a pointer",
        )?;
        replace_call_with_value(args.instr, value)
    }

    fn lower_dynamic_qubit_release(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let helper = ensure_dynamic_qubit_release(args.ctx, module_ref(args));
        lower_void_helper_call(
            args.ctx,
            args.instr,
            helper,
            &call_args[..1],
            "Failed to lower dynamic qubit release",
        )
    }

    fn lower_dynamic_qubit_array_allocate(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let helper = ensure_dynamic_qubit_array_allocate(args.ctx, module_ref(args));
        lower_void_helper_call(
            args.ctx,
            args.instr,
            helper,
            &call_args[..3],
            "Failed to lower dynamic qubit array allocation",
        )
    }

    fn lower_dynamic_qubit_array_release(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let helper = ensure_dynamic_qubit_array_release(args.ctx, module_ref(args));
        lower_void_helper_call(
            args.ctx,
            args.instr,
            helper,
            &call_args[..2],
            "Failed to lower dynamic qubit array release",
        )
    }

    fn ensure_dynamic_result_setter<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.result_set_pending") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let slot_type = get_dynamic_result_slot_type(ctx);
        let function = module.add_function(
            "qir_qis.result_set_pending",
            ctx.void_type()
                .fn_type(&[ptr_type.into(), ctx.i64_type().into()], false),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        builder.position_at_end(entry);
        let result_ptr = function
            .get_nth_param(0)
            .expect("result")
            .into_pointer_value();
        let future = function.get_nth_param(1).expect("future").into_int_value();
        let slot_ptrs = get_dynamic_result_slot_ptrs(&builder, slot_type, result_ptr);
        set_dynamic_result_pending(ctx, &builder, &slot_ptrs, future);
        builder.build_return(None).expect("ret");
        function
    }

    fn ensure_dynamic_result_read<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.result_read") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let slot_type = get_dynamic_result_slot_type(ctx);
        let function = module.add_function(
            "qir_qis.result_read",
            ctx.bool_type().fn_type(&[ptr_type.into()], false),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        let ret_false = ctx.append_basic_block(function, "ret_false");
        let pending = ctx.append_basic_block(function, "pending");
        let cached = ctx.append_basic_block(function, "cached");
        builder.position_at_end(entry);
        let result_ptr = function
            .get_first_param()
            .expect("result")
            .into_pointer_value();
        let is_null =
            build_ptr_is_null(ctx, &builder, result_ptr, "result_int").expect("null check");
        let read_state = ctx.append_basic_block(function, "read_state");
        builder
            .build_conditional_branch(is_null, ret_false, read_state)
            .expect("branch");

        builder.position_at_end(read_state);
        let slot_ptrs = get_dynamic_result_slot_ptrs(&builder, slot_type, result_ptr);
        let state = builder
            .build_load(ctx.i8_type(), slot_ptrs.state, "state")
            .expect("load state")
            .into_int_value();
        let is_pending = builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                state,
                ctx.i8_type().const_int(1, false),
                "is_pending",
            )
            .expect("cmp");
        let is_cached = builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                state,
                ctx.i8_type().const_int(2, false),
                "is_cached",
            )
            .expect("cmp");
        let check_cached = ctx.append_basic_block(function, "check_cached");
        builder
            .build_conditional_branch(is_pending, pending, check_cached)
            .expect("branch");

        builder.position_at_end(check_cached);
        builder
            .build_conditional_branch(is_cached, cached, ret_false)
            .expect("branch");

        builder.position_at_end(pending);
        build_dynamic_result_pending_return(
            ctx,
            module,
            &builder,
            slot_ptrs.future,
            slot_ptrs.cached,
            slot_ptrs.state,
        );

        builder.position_at_end(cached);
        let cached_val = builder
            .build_load(ctx.bool_type(), slot_ptrs.cached, "cached")
            .expect("cached load");
        builder
            .build_return(Some(&cached_val.into_int_value()))
            .expect("ret cached");

        builder.position_at_end(ret_false);
        builder
            .build_return(Some(&ctx.bool_type().const_zero()))
            .expect("ret false");
        function
    }

    fn ensure_dynamic_result_release<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.result_release") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let slot_type = get_dynamic_result_slot_type(ctx);
        let function = module.add_function(
            "qir_qis.result_release",
            ctx.void_type().fn_type(&[ptr_type.into()], false),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        let ret = ctx.append_basic_block(function, "ret");
        let body = ctx.append_basic_block(function, "body");
        builder.position_at_end(entry);
        let result_ptr = function
            .get_first_param()
            .expect("result")
            .into_pointer_value();
        let is_null =
            build_ptr_is_null(ctx, &builder, result_ptr, "result_int").expect("null check");
        builder
            .build_conditional_branch(is_null, ret, body)
            .expect("branch");
        builder.position_at_end(body);
        let slot_ptrs = get_dynamic_result_slot_ptrs(&builder, slot_type, result_ptr);
        let state = builder
            .build_load(ctx.i8_type(), slot_ptrs.state, "state")
            .expect("load state")
            .into_int_value();
        let is_pending = builder
            .build_int_compare(
                inkwell::IntPredicate::EQ,
                state,
                ctx.i8_type().const_int(1, false),
                "is_pending",
            )
            .expect("cmp");
        let dec_block = ctx.append_basic_block(function, "dec_future");
        let clear_block = ctx.append_basic_block(function, "clear");
        builder
            .build_conditional_branch(is_pending, dec_block, clear_block)
            .expect("branch");
        builder.position_at_end(dec_block);
        let future = builder
            .build_load(ctx.i64_type(), slot_ptrs.future, "future")
            .expect("load future");
        let dec_fn = get_or_create_function(
            module,
            "___dec_future_refcount",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );
        let _ = builder.build_call(dec_fn, &[future.into()], "");
        builder
            .build_unconditional_branch(clear_block)
            .expect("jump");
        builder.position_at_end(clear_block);
        clear_dynamic_result_slot(ctx, &builder, &slot_ptrs);
        builder.build_unconditional_branch(ret).expect("jump");
        builder.position_at_end(ret);
        builder.build_return(None).expect("ret");
        function
    }

    fn ensure_dynamic_result_array_release<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> FunctionValue<'ctx> {
        if let Some(existing) = module.get_function("qir_qis.result_array_release") {
            return existing;
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let function = module.add_function(
            "qir_qis.result_array_release",
            ctx.void_type()
                .fn_type(&[ctx.i64_type().into(), ptr_type.into()], false),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        builder.position_at_end(entry);
        let len = function.get_nth_param(0).expect("len").into_int_value();
        let array_ptr = function
            .get_nth_param(1)
            .expect("array")
            .into_pointer_value();
        let release_fn = ensure_dynamic_result_release(ctx, module);
        let ptr_elem_type = ctx.ptr_type(AddressSpace::default());
        build_pointer_array_release_helper(
            ctx,
            &builder,
            &PointerArrayReleaseHelperArgs {
                function,
                len,
                array_ptr,
                release_fn,
                ptr_elem_type,
                gep_error: "Failed to build result array GEP",
                load_error: "Failed to load dynamic result pointer",
                release_error: "Failed to release dynamic result",
            },
        )
        .expect("build dynamic result release loop");
        builder.build_return(None).expect("ret");
        function
    }

    fn ensure_dynamic_result_array_record_output<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
    ) -> Result<FunctionValue<'ctx>, String> {
        if let Some(existing) = module.get_function("qir_qis.result_array_record_output") {
            return Ok(existing);
        }
        let ptr_type = ctx.ptr_type(AddressSpace::default());
        let function = module.add_function(
            "qir_qis.result_array_record_output",
            ctx.void_type().fn_type(
                &[
                    ctx.i64_type().into(),
                    ptr_type.into(),
                    ptr_type.into(),
                    ctx.i64_type().into(),
                ],
                false,
            ),
            Some(Linkage::Private),
        );
        let builder = ctx.create_builder();
        let entry = ctx.append_basic_block(function, "entry");
        builder.position_at_end(entry);
        let len = function.get_nth_param(0).expect("len").into_int_value();
        let array_ptr = function
            .get_nth_param(1)
            .expect("array")
            .into_pointer_value();
        let tag_ptr = function.get_nth_param(2).expect("tag").into_pointer_value();
        let tag_len = function.get_nth_param(3).expect("tag_len").into_int_value();
        let print_bool_arr = get_or_create_function(
            module,
            "print_bool_arr",
            ctx.void_type().fn_type(
                &[ptr_type.into(), ctx.i64_type().into(), ptr_type.into()],
                false,
            ),
        );
        let read_fn = ensure_dynamic_result_read(ctx, module);
        let bool_arr =
            build_dynamic_result_array_values(ctx, function, &builder, len, array_ptr, read_fn)?;
        let array_desc = build_dynamic_result_array_descriptor(ctx, &builder, len, bool_arr)?;
        let _ = builder
            .build_call(
                print_bool_arr,
                &[tag_ptr.into(), tag_len.into(), array_desc.into()],
                "",
            )
            .map_err(|e| format!("Failed to print result array: {e}"))?;
        builder.build_return(None).expect("ret");
        Ok(function)
    }

    fn lower_dynamic_result_allocate(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let builder = args.ctx.create_builder();
        builder.position_before(&args.instr);
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let slot_ptr = builder
            .build_alloca(get_dynamic_result_slot_type(args.ctx), "dyn_result")
            .map_err(|e| format!("Failed to allocate dynamic result slot: {e}"))?;
        initialize_dynamic_result_slot(args.ctx, &builder, slot_ptr);
        let helper = ensure_out_err_success(args.ctx, module_ref(args));
        let _ = builder
            .build_call(helper, &[call_args[0].into()], "")
            .map_err(|e| format!("Failed to store dynamic result out_err success flag: {e}"))?;
        replace_call_with_value(args.instr, slot_ptr.as_basic_value_enum())
    }

    fn lower_dynamic_result_release(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let helper = ensure_dynamic_result_release(args.ctx, module_ref(args));
        lower_void_helper_call(
            args.ctx,
            args.instr,
            helper,
            &call_args[..1],
            "Failed to lower dynamic result release",
        )
    }

    fn lower_dynamic_result_array_allocate(args: &mut ProcessCallArgs<'_>) -> Result<(), String> {
        let builder = args.ctx.create_builder();
        builder.position_before(&args.instr);
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let len = extract_const_len(call_args[0], "__quantum__rt__result_array_allocate")?;
        let array_ptr = call_args[1].into_pointer_value();
        let out_err = call_args[2].into_pointer_value();
        let ptr_type = args.ctx.ptr_type(AddressSpace::default());
        for idx in 0..len {
            let elem_ptr = unsafe {
                builder.build_gep(
                    ptr_type,
                    array_ptr,
                    &[args.ctx.i64_type().const_int(idx, false)],
                    "result_elem_ptr",
                )
            }
            .map_err(|e| format!("Failed to build result array element GEP: {e}"))?;
            let slot_ptr = builder
                .build_alloca(get_dynamic_result_slot_type(args.ctx), "dyn_result")
                .map_err(|e| format!("Failed to allocate dynamic result slot: {e}"))?;
            initialize_dynamic_result_slot(args.ctx, &builder, slot_ptr);
            let _ = builder
                .build_store(elem_ptr, slot_ptr)
                .map_err(|e| format!("Failed to store dynamic result slot in array: {e}"))?;
        }
        let helper = ensure_out_err_success(args.ctx, module_ref(args));
        let _ = builder
            .build_call(helper, &[out_err.into()], "")
            .map_err(|e| {
                format!("Failed to store dynamic result array out_err success flag: {e}")
            })?;
        args.instr.erase_from_basic_block();
        Ok(())
    }

    fn lower_dynamic_result_array_release(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let helper = ensure_dynamic_result_array_release(args.ctx, module_ref(args));
        lower_void_helper_call(
            args.ctx,
            args.instr,
            helper,
            &call_args[..2],
            "Failed to lower dynamic result array release",
        )
    }

    fn lower_dynamic_result_array_record_output(
        args: &mut ProcessCallArgs<'_>,
    ) -> Result<(), String> {
        let builder = args.ctx.create_builder();
        builder.position_before(&args.instr);
        let call_args: Vec<BasicValueEnum> = extract_operands(&args.instr)?;
        let old_name = parse_gep(call_args[2])?;
        let full_tag =
            if let Some(global) = unsafe { &mut *args.global_mapping }.get(old_name.as_str()) {
                get_string_label(*global)?
            } else {
                return Err(format!("Output global `{old_name}` not found in mapping"));
            };
        let old_label = full_tag
            .rfind(':')
            .and_then(|pos| pos.checked_add(1))
            .map_or_else(|| full_tag.clone(), |pos| full_tag[pos..].to_string());
        let (new_const, new_name) =
            build_result_global(args.ctx, &old_label, &old_name, "RESULT_ARRAY", None)?;
        let new_global = module_ref(args).add_global(new_const.get_type(), None, &new_name);
        new_global.set_initializer(&new_const);
        new_global.set_linkage(Linkage::Private);
        new_global.set_constant(true);
        unsafe { &mut *args.global_mapping }.insert(old_name, new_global);
        let helper = ensure_dynamic_result_array_record_output(args.ctx, module_ref(args))?;
        let tag_len = args.ctx.i64_type().const_int(
            u64::from(new_const.get_type().len()).saturating_sub(1),
            false,
        );
        let tag_ptr = unsafe {
            builder.build_gep(
                new_const.get_type(),
                new_global.as_pointer_value(),
                &[
                    args.ctx.i64_type().const_zero(),
                    args.ctx.i64_type().const_zero(),
                ],
                "tag_gep",
            )
        }
        .map_err(|e| format!("Failed to build result array tag GEP: {e}"))?;
        let _ = builder
            .build_call(
                helper,
                &[
                    call_args[0].into(),
                    call_args[1].into(),
                    tag_ptr.into(),
                    tag_len.into(),
                ],
                "",
            )
            .map_err(|e| format!("Failed to lower result array record output: {e}"))?;
        args.instr.erase_from_basic_block();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_mz_call<'ctx>(
        ctx: &'ctx Context,
        module: *const (),
        instr: &'ctx inkwell::values::InstructionValue<'ctx>,
        fn_name: &str,
        capability_flags: CapabilityFlags,
        qubit_array: Option<PointerValue<'ctx>>,
        qubit_array_type: Option<ArrayType<'ctx>>,
        result_ssa: *mut (),
    ) -> Result<(), String> {
        let module = unsafe { &*module.cast::<Module<'ctx>>() };
        if fn_name == "__quantum__qis__m__body" {
            log::warn!(
                "`__quantum__qis__m__body` is from Q# QDK, synonym for `__quantum__qis__mz__body`"
            );
        }
        let builder = ctx.create_builder();
        builder.position_before(instr);

        // Extract qubit and result indices
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let qubit_ptr = call_args[0].into_pointer_value();
        let result_ptr = call_args[1].into_pointer_value();

        let q_handle = get_qubit_handle(
            ctx,
            capability_flags,
            qubit_array,
            qubit_array_type,
            &builder,
            qubit_ptr,
        )?;

        // Create ___lazy_measure call
        let meas = {
            let meas_func = get_or_create_function(
                module,
                "___lazy_measure",
                ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
            );

            let call = builder.build_call(meas_func, &[q_handle.into()], "meas");
            let call_result =
                call.map_err(|e| format!("Failed to build call for lazy measure function: {e}"))?;
            match call_result.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err("Failed to get basic value from lazy measure call".into());
                }
            }
        };

        // Store measurement result
        if capability_flags.dynamic_result_management {
            let set_result_fn = ensure_dynamic_result_setter(ctx, module);
            let _ = builder
                .build_call(set_result_fn, &[result_ptr.into(), meas.into()], "")
                .map_err(|e| format!("Failed to update dynamic result state: {e}"))?;
        } else {
            let result_ssa = unsafe {
                &mut *result_ssa
                    .cast::<Vec<Option<(BasicValueEnum<'ctx>, Option<BasicValueEnum<'ctx>>)>>>()
            };
            let result_idx = get_index(result_ptr)?;
            let result_idx_usize = checked_result_index(result_idx, result_ssa.len())?;
            result_ssa[result_idx_usize] = Some((meas, None));
        }

        if fn_name == "__quantum__qis__mresetz__body" {
            log::warn!("`__quantum__qis__mresetz__body` is from Q# QDK");
            // Create ___reset call
            create_reset_call(ctx, module, &builder, q_handle);
        }

        // Remove original call
        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_mz_leaked_call(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let builder = ctx.create_builder();
        builder.position_before(instr);
        let call = CallSiteValue::try_from(args.instr)
            .map_err(|()| "Malformed mz_leaked call: instruction is not a call site".to_string())?;
        let called_fn = call
            .get_called_fn_value()
            .ok_or_else(|| "Malformed mz_leaked call: missing callee".to_string())?;
        let fn_type = called_fn.get_type();
        let param_types = fn_type.get_param_types();
        let has_expected_signature = fn_type
            .get_return_type()
            .is_some_and(|ty| ty.is_int_type() && ty.into_int_type().get_bit_width() == 64)
            && param_types.len() == 1
            && param_types[0].is_pointer_type();
        if !has_expected_signature {
            return Err("Malformed mz_leaked call: expected signature i64 (ptr)".to_string());
        }

        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let qubit_ptr = mz_leaked_qubit_operand(&call_args)?;

        let q_handle = {
            let idx_fn = module
                .get_function(LOAD_QUBIT_FN)
                .ok_or_else(|| format!("{LOAD_QUBIT_FN} not found"))?;
            let idx_call = builder
                .build_call(idx_fn, &[qubit_ptr.into()], "qbit")
                .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}"))?;
            match idx_call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err(format!(
                        "Failed to get basic value from {LOAD_QUBIT_FN} call"
                    ));
                }
            }
        };

        let meas_handle = {
            let meas_func = get_or_create_function(
                module,
                "___lazy_measure_leaked",
                ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
            );

            let call = builder.build_call(meas_func, &[q_handle.into()], "meas_leaked");
            let call_result = call.map_err(|e| {
                format!("Failed to build call for lazy leaked measure function: {e}")
            })?;
            match call_result.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err("Failed to get basic value from lazy leaked measure call".into());
                }
            }
        };

        let meas_value = {
            let read_func = get_or_create_function(
                module,
                "___read_future_uint",
                ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
            );
            let call = builder.build_call(read_func, &[meas_handle.into()], "meas_leaked_value");
            let call_result =
                call.map_err(|e| format!("Failed to build call for read_future_uint: {e}"))?;
            match call_result.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err("Failed to get basic value from read_future_uint call".into());
                }
            }
        };

        let dec_func = get_or_create_function(
            module,
            "___dec_future_refcount",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );
        let _ = builder
            .build_call(dec_func, &[meas_handle.into()], "")
            .map_err(|e| format!("Failed to build call for dec_future_refcount: {e}"))?;

        let instruction_val = meas_value
            .as_instruction_value()
            .ok_or("Failed to convert leaked measurement value to instruction value")?;
        instr.replace_all_uses_with(&instruction_val);
        instr.erase_from_basic_block();
        Ok(())
    }

    pub fn mz_leaked_qubit_operand<'ctx>(
        call_args: &[BasicValueEnum<'ctx>],
    ) -> Result<PointerValue<'ctx>, String> {
        match call_args {
            [BasicValueEnum::PointerValue(ptr), _] => Ok(*ptr),
            [_, _] => {
                Err("Malformed mz_leaked call: expected first argument to be a pointer".into())
            }
            _ => Err(format!(
                "Malformed mz_leaked call: expected 1 argument plus callee, got {} operands",
                call_args.len()
            )),
        }
    }

    fn handle_reset_call(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let builder = ctx.create_builder();
        builder.position_before(instr);

        // Extract qubit index
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let qubit_ptr = call_args[0].into_pointer_value();

        let q_handle = get_qubit_handle(
            ctx,
            args.capability_flags,
            args.qubit_array,
            args.qubit_array_type,
            &builder,
            qubit_ptr,
        )?;

        // Create ___reset call
        create_reset_call(ctx, module, &builder, q_handle);

        instr.erase_from_basic_block();
        Ok(())
    }

    fn parse_barrier_arity(fn_name: &str) -> Result<usize, String> {
        fn_name
            .strip_prefix("__quantum__qis__barrier")
            .and_then(|s| s.strip_suffix("__body"))
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .ok_or_else(|| format!("Invalid barrier function name: {fn_name}"))
    }

    #[allow(clippy::too_many_lines)]
    fn handle_barrier_call(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs {
            ctx,
            instr,
            fn_name,
            ..
        } = args;
        let module = module_ref(args);
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let num_qubits = parse_barrier_arity(fn_name)?;

        // Extract qubit arguments (excluding the last operand which is the function pointer)
        let all_operands: Vec<BasicValueEnum> = extract_operands(instr)?;
        let num_operands = all_operands
            .len()
            .checked_sub(1)
            .ok_or("Expected at least one operand")?;

        if num_operands != num_qubits {
            return Err(format!(
                "Barrier function {fn_name} expects {num_qubits} arguments, got {num_operands}"
            ));
        }

        let call_args = &all_operands[..num_operands];

        // Load qubit handles into an array
        let i64_type = ctx.i64_type();
        let array_type = i64_type.array_type(
            u32::try_from(num_qubits).map_err(|e| format!("Failed to convert num_qubits: {e}"))?,
        );
        let array_alloca = builder
            .build_alloca(array_type, "barrier_qubits")
            .map_err(|e| format!("Failed to allocate array for barrier qubits: {e}"))?;

        for (i, arg) in call_args.iter().enumerate() {
            let qubit_ptr = arg.into_pointer_value();
            let q_handle = get_qubit_handle(
                ctx,
                args.capability_flags,
                args.qubit_array,
                args.qubit_array_type,
                &builder,
                qubit_ptr,
            )?;

            let elem_ptr = unsafe {
                builder.build_gep(
                    array_type,
                    array_alloca,
                    &[
                        i64_type.const_zero(),
                        i64_type.const_int(
                            u64::try_from(i)
                                .map_err(|e| format!("Failed to convert index: {e}"))?,
                            false,
                        ),
                    ],
                    "",
                )
            }
            .map_err(|e| format!("Failed to build GEP for barrier array: {e}"))?;
            builder
                .build_store(elem_ptr, q_handle)
                .map_err(|e| format!("Failed to store qubit handle in array: {e}"))?;
        }

        let array_ptr = unsafe {
            builder.build_gep(
                array_type,
                array_alloca,
                &[i64_type.const_zero(), i64_type.const_zero()],
                "barrier_array_ptr",
            )
        }
        .map_err(|e| format!("Failed to build GEP for barrier array pointer: {e}"))?;

        // void ___barrier(i64* %qbs, i64 %qbs_len)
        let barrier_func = get_or_create_function(
            module,
            "___barrier",
            ctx.void_type().fn_type(
                &[
                    ctx.ptr_type(AddressSpace::default()).into(),
                    i64_type.into(),
                ],
                false,
            ),
        );

        builder
            .build_call(
                barrier_func,
                &[
                    array_ptr.into(),
                    i64_type
                        .const_int(
                            u64::try_from(num_qubits)
                                .map_err(|e| format!("Failed to convert num_qubits: {e}"))?,
                            false,
                        )
                        .into(),
                ],
                "",
            )
            .map_err(|e| format!("Failed to build call to ___barrier: {e}"))?;

        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_read_result_call<'ctx>(
        ctx: &'ctx Context,
        module: *const (),
        instr: &'ctx inkwell::values::InstructionValue<'ctx>,
        fn_name: &str,
        capability_flags: CapabilityFlags,
        global_mapping: *mut (),
        result_ssa: *mut (),
    ) -> Result<(), String> {
        let module = unsafe { &*module.cast::<Module<'ctx>>() };
        let global_mapping = unsafe {
            &mut *global_mapping.cast::<HashMap<String, inkwell::values::GlobalValue<'ctx>>>()
        };
        let result_ssa = unsafe {
            &mut *result_ssa
                .cast::<Vec<Option<(BasicValueEnum<'ctx>, Option<BasicValueEnum<'ctx>>)>>>()
        };
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let result_ptr = call_args[0].into_pointer_value();

        let builder = ctx.create_builder();
        builder.position_before(instr);

        let bool_val = read_result_bool(
            ctx,
            module,
            &builder,
            result_ptr,
            capability_flags,
            result_ssa,
        )?;

        if fn_name == "__quantum__rt__read_result" {
            let instruction_val = bool_val
                .as_instruction_value()
                .ok_or("Failed to convert bool_val to instruction value")?;
            instr.replace_all_uses_with(&instruction_val);
        } else {
            let new_global = get_or_create_result_output_global(
                ctx,
                module,
                global_mapping,
                capability_flags,
                result_ptr,
                call_args[1],
            )?;

            let print_func = get_or_create_function(
                module,
                "print_bool",
                ctx.void_type().fn_type(
                    &[
                        ctx.ptr_type(AddressSpace::default()).into(), // ptr
                        ctx.i64_type().into(),                        // i64
                        ctx.bool_type().into(),                       // i1
                    ],
                    false,
                ),
            );

            add_print_call(ctx, &builder, new_global, print_func, bool_val)?;
        }
        instr.erase_from_basic_block();
        Ok(())
    }

    pub fn checked_result_index(result_idx: u64, result_ssa_len: usize) -> Result<usize, String> {
        let result_idx_usize = usize::try_from(result_idx)
            .map_err(|e| format!("Failed to convert result index to usize: {e}"))?;
        if result_idx_usize >= result_ssa_len {
            return Err(format!(
                "Result index {result_idx} exceeds required_num_results ({result_ssa_len})"
            ));
        }
        Ok(result_idx_usize)
    }

    fn handle_classical_record_output(args: &mut ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs {
            ctx,
            instr,
            fn_name,
            ..
        } = args;
        let module = unsafe { &*args.module };
        let global_mapping = unsafe { &mut *args.global_mapping };
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let (print_func_name, value, type_tag) = match fn_name.as_str() {
            "__quantum__rt__bool_record_output" => (
                "print_bool",
                call_args[0].into_int_value().as_basic_value_enum(),
                "BOOL",
            ),
            "__quantum__rt__int_record_output" => (
                "print_int",
                call_args[0].into_int_value().as_basic_value_enum(),
                "INT",
            ),
            "__quantum__rt__double_record_output" => (
                "print_float",
                call_args[0].into_float_value().as_basic_value_enum(),
                "FLOAT",
            ),
            _ => unreachable!(),
        };

        // Get the print function type based on the value type
        let ret_type = ctx.void_type();
        let param_types = &[
            ctx.ptr_type(AddressSpace::default()).into(), // ptr
            ctx.i64_type().into(),                        // i64
            match type_tag {
                "BOOL" => ctx.bool_type().into(),
                "INT" => ctx.i64_type().into(),
                "FLOAT" => ctx.f64_type().into(),
                _ => unreachable!(),
            },
        ];
        let fn_type = ret_type.fn_type(param_types, false);

        let print_func = get_or_create_function(module, print_func_name, fn_type);

        let parsed_name = parse_gep(call_args[1]);
        let old_name = parsed_name.clone().unwrap_or_else(|_| {
            format!(
                "anon_classical_{}",
                match type_tag {
                    "BOOL" => "bool",
                    "INT" => "int",
                    "FLOAT" => "float",
                    _ => "value",
                }
            )
        });

        let full_tag = if let Some(existing) = global_mapping.get(old_name.as_str()) {
            get_string_label(*existing)?
        } else if parsed_name.is_ok() {
            return Err(format!("Output global `{old_name}` not found in mapping"));
        } else {
            old_name.clone()
        };
        // Parse the label from the global string (format: USER:RESULT:tag)
        let old_label = full_tag
            .rfind(':')
            .and_then(|pos| pos.checked_add(1))
            .map_or_else(|| full_tag.clone(), |pos| full_tag[pos..].to_string());

        let (new_const, new_name) =
            build_result_global(ctx, &old_label, &old_name, type_tag, None)?;

        let new_global = module.add_global(new_const.get_type(), None, &new_name);
        new_global.set_initializer(&new_const);
        new_global.set_linkage(inkwell::module::Linkage::Private);
        new_global.set_constant(true);
        global_mapping.insert(old_name, new_global);
        record_classical_output(ctx, *instr, new_global, print_func, value)?;
        Ok(())
    }

    fn handle_get_current_shot(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let get_shot_func = get_or_create_function(
            module,
            "get_current_shot",
            ctx.i64_type().fn_type(&[], false),
        );
        handle_runtime_value_call(
            ctx,
            *instr,
            get_shot_func,
            &[],
            "current_shot",
            "Failed to build call to get_current_shot",
            "Failed to get basic value from get_current_shot call",
        )
    }

    fn handle_runtime_value_call<'ctx>(
        ctx: &'ctx Context,
        instr: inkwell::values::InstructionValue<'ctx>,
        callee: FunctionValue<'ctx>,
        call_args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
        build_error: &str,
        value_error: &str,
    ) -> Result<(), String> {
        let builder = ctx.create_builder();
        builder.position_before(&instr);
        let value = call_basic_value(&builder, callee, call_args, name, build_error, value_error)?;
        if let Some(instr_val) = value.as_instruction_value() {
            instr.replace_all_uses_with(&instr_val);
        }
        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_runtime_void_call<'ctx>(
        ctx: &'ctx Context,
        instr: inkwell::values::InstructionValue<'ctx>,
        callee: FunctionValue<'ctx>,
        call_args: &[BasicMetadataValueEnum<'ctx>],
        build_error: &str,
    ) -> Result<(), String> {
        let builder = ctx.create_builder();
        builder.position_before(&instr);
        let _ = builder
            .build_call(callee, call_args, "")
            .map_err(|e| format!("{build_error}: {e}"))?;
        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_random_seed(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let random_seed_func = get_or_create_function(
            module,
            "random_seed",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );
        handle_runtime_void_call(
            ctx,
            *instr,
            random_seed_func,
            &[call_args[0].into()],
            "Failed to build call to random_seed",
        )
    }

    fn handle_random_int(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let random_int_func =
            get_or_create_function(module, "random_int", ctx.i32_type().fn_type(&[], false));
        handle_runtime_value_call(
            ctx,
            *instr,
            random_int_func,
            &[],
            "rint",
            "Failed to build call to random_int",
            "Failed to get basic value from random_int call",
        )
    }

    fn handle_random_float(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let random_float_func =
            get_or_create_function(module, "random_float", ctx.f64_type().fn_type(&[], false));
        handle_runtime_value_call(
            ctx,
            *instr,
            random_float_func,
            &[],
            "rfloat",
            "Failed to build call to random_float",
            "Failed to get basic value from random_float call",
        )
    }

    fn handle_random_int_bounded(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let random_rng_func = get_or_create_function(
            module,
            "random_rng",
            ctx.i32_type().fn_type(&[ctx.i32_type().into()], false),
        );
        handle_runtime_value_call(
            ctx,
            *instr,
            random_rng_func,
            &[call_args[0].into()],
            "rintb",
            "Failed to build call to random_rng",
            "Failed to get basic value from random_rng call",
        )
    }

    fn handle_random_advance(args: &ProcessCallArgs<'_>) -> Result<(), String> {
        let ProcessCallArgs { ctx, instr, .. } = args;
        let module = module_ref(args);
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let random_advance_func = get_or_create_function(
            module,
            "random_advance",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );
        handle_runtime_void_call(
            ctx,
            *instr,
            random_advance_func,
            &[call_args[0].into()],
            "Failed to build call to random_advance",
        )
    }
}

pub(crate) fn decode_llvm_bytes(value: &[u8]) -> Option<&str> {
    std::str::from_utf8(value).ok()
}

pub(crate) fn decode_llvm_c_string(value: &std::ffi::CStr) -> Option<&str> {
    value.to_str().ok()
}

fn create_memory_buffer_from_bytes(
    bytes: &[u8],
    name: &str,
) -> Result<inkwell::memory_buffer::MemoryBuffer<'static>, String> {
    use llvm_sys::core::LLVMCreateMemoryBufferWithMemoryRangeCopy;

    let name = std::ffi::CString::new(name)
        .map_err(|_| "Memory buffer name contains interior NUL byte".to_string())?;
    let memory_buffer = unsafe {
        LLVMCreateMemoryBufferWithMemoryRangeCopy(bytes.as_ptr().cast(), bytes.len(), name.as_ptr())
    };
    if memory_buffer.is_null() {
        return Err(
            "LLVM failed to create memory buffer from bytes: received null memory buffer pointer"
                .to_string(),
        );
    }

    unsafe { Ok(inkwell::memory_buffer::MemoryBuffer::new(memory_buffer)) }
}

pub(crate) fn create_module_from_ir_text<'ctx>(
    ctx: &'ctx inkwell::context::Context,
    ll_text: &str,
    name: &str,
) -> Result<inkwell::module::Module<'ctx>, String> {
    let memory_buffer = create_memory_buffer_from_bytes(ll_text.as_bytes(), name)?;
    ctx.create_module_from_ir(memory_buffer)
        .map_err(|e| format!("Failed to create module from LLVM IR: {e}"))
}

fn memory_buffer_to_owned_bytes(
    memory_buffer: &inkwell::memory_buffer::MemoryBuffer<'_>,
) -> Vec<u8> {
    let bytes = memory_buffer.as_slice();
    if bytes.last() == Some(&0) {
        bytes[..bytes.len().saturating_sub(1)].to_vec()
    } else {
        bytes.to_vec()
    }
}

pub(crate) fn parse_bitcode_module<'ctx>(
    ctx: &'ctx inkwell::context::Context,
    bitcode: &[u8],
    name: &str,
) -> Result<inkwell::module::Module<'ctx>, String> {
    let memory_buffer = create_memory_buffer_from_bytes(bitcode, name)?;
    inkwell::module::Module::parse_bitcode_from_buffer(&memory_buffer, ctx)
        .map_err(|e| format!("Failed to parse bitcode: {e}"))
}

/// Core QIR to QIS translation logic.
///
/// # Arguments
/// - `bc_bytes` - The QIR bytes to translate.
/// - `opt_level` - The optimization level to use (0-3). Platform defaults are
///   exposed via [`DEFAULT_OPT_LEVEL`].
/// - `target` - Target architecture ("aarch64", "x86-64", "native"). Platform
///   defaults are exposed via [`DEFAULT_TARGET`].
/// - `wasm_bytes` - Optional WASM bytes for Wasm codegen.
///
/// # Errors
/// Returns an error string if the translation fails.
pub fn qir_to_qis(
    bc_bytes: &[u8],
    opt_level: u32,
    target: &str,
    _wasm_bytes: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    use crate::{
        aux::{get_capability_flags, process_entry_function},
        convert::{
            add_qmain_wrapper, create_qubit_array, find_entry_function, free_all_qubits,
            get_string_attrs, process_ir_defined_q_fns, prune_unused_ir_qis_helpers,
        },
        decompose::add_decompositions,
        opt::optimize,
        utils::add_generator_metadata,
    };
    use inkwell::{attributes::AttributeLoc, context::Context};
    use std::{collections::BTreeMap, env};

    let ctx = Context::create();
    let module = parse_bitcode_module(&ctx, bc_bytes, "bitcode")?;
    crate::llvm_verify::verify_module(&module, "LLVM module verification failed after parse")?;

    add_decompositions(&ctx, &module)
        .map_err(|e| format!("Failed to add QIR decompositions: {e}"))?;
    let entry_fn = find_entry_function(&module)
        .map_err(|e| format!("Failed to find entry function in QIR module: {e}"))?;

    let entry_fn_name = entry_fn
        .get_name()
        .to_str()
        .map_err(|e| format!("Invalid UTF-8 in entry function name: {e}"))?;
    let capability_flags = get_capability_flags(&module);

    log::trace!("Entry function: {entry_fn_name}");
    let new_name = format!("___user_qir_{entry_fn_name}");
    entry_fn.as_global_value().set_name(&new_name);
    log::debug!("Renamed entry function to: {new_name}");
    let qubit_array = if capability_flags.dynamic_qubit_management {
        None
    } else {
        Some(create_qubit_array(&ctx, &module, entry_fn)?)
    };

    let wasm_fns: BTreeMap<String, u64> = BTreeMap::new();
    process_entry_function(
        &ctx,
        &module,
        entry_fn,
        &wasm_fns,
        qubit_array,
        capability_flags,
    )?;

    // Handle IR defined functions that take qubits
    process_ir_defined_q_fns(
        &ctx,
        &module,
        entry_fn,
        capability_flags.dynamic_qubit_management,
    )?;

    if let Some(qubit_array) = qubit_array {
        free_all_qubits(&ctx, &module, entry_fn, qubit_array)?;
    }

    // Add qmain wrapper that calls setup, entry function, and teardown
    let _ = add_qmain_wrapper(&ctx, &module, entry_fn);

    crate::llvm_verify::verify_module(&module, "LLVM module verification failed")?;

    // Clean up the translated module
    for attr in get_string_attrs(entry_fn) {
        let kind = decode_string_attribute_kind(attr)?;
        entry_fn.remove_string_attribute(AttributeLoc::Function, &kind);
    }

    // TODO: remove global module metadata
    // seems inkwell doesn't support this yet

    // Add metadata to the module
    let md_string = ctx.metadata_string("mainlib");
    let md_node = ctx.metadata_node(&[md_string.into()]);
    module
        .add_global_metadata("name", &md_node)
        .map_err(|e| format!("Failed to add global metadata: {e}"))?;
    add_generator_metadata(&ctx, &module, "gen_name", env!("CARGO_PKG_NAME"))?;
    add_generator_metadata(&ctx, &module, "gen_version", env!("CARGO_PKG_VERSION"))?;

    optimize(&module, opt_level, target)?;
    prune_unused_ir_qis_helpers(&module);

    Ok(memory_buffer_to_owned_bytes(
        &module.write_bitcode_to_memory(),
    ))
}

/// Extract WASM function mapping from the given WASM bytes.
///
/// # Errors
/// Returns an error string if parsing fails.
#[cfg(feature = "wasm")]
pub fn get_wasm_functions(
    wasm_bytes: Option<&[u8]>,
) -> Result<std::collections::BTreeMap<String, u64>, String> {
    use crate::utils::parse_wasm_functions;
    use std::collections::BTreeMap;

    let mut wasm_fns: BTreeMap<String, u64> = BTreeMap::new();
    if let Some(bytes) = wasm_bytes {
        wasm_fns = parse_wasm_functions(bytes)?;
        log::debug!("WASM function map: {wasm_fns:?}");
    }
    Ok(wasm_fns)
}

#[cfg(not(feature = "wasm"))]
fn get_wasm_functions(
    _wasm_bytes: Option<&[u8]>,
) -> Result<std::collections::BTreeMap<String, u64>, String> {
    Ok(std::collections::BTreeMap::new())
}

/// Validate the given QIR bitcode.
///
/// # Arguments
/// - `bc_bytes` - The QIR bytes to validate.
/// - `wasm_bytes` - Optional WASM bytes to validate against.
///
/// # Errors
/// Returns an error string if validation fails.
pub fn validate_qir(bc_bytes: &[u8], wasm_bytes: Option<&[u8]>) -> Result<(), String> {
    use crate::{
        aux::{
            get_capability_flags, validate_capability_usage,
            validate_dynamic_array_allocation_backing,
            validate_dynamic_result_allocation_placement, validate_functions,
            validate_module_flags, validate_module_layout_and_triple, validate_result_slot_usage,
        },
        convert::{ENTRY_ATTRIBUTE_KEYS, find_entry_function},
    };
    use inkwell::{attributes::AttributeLoc, context::Context};

    let ctx = Context::create();
    let module = parse_bitcode_module(&ctx, bc_bytes, "bitcode")?;
    let mut errors = Vec::new();

    let capability_flags = get_capability_flags(&module);
    validate_module_layout_and_triple(&module);
    let entry_fn = if let Ok(entry_fn) = find_entry_function(&module) {
        if entry_fn.get_basic_blocks().is_empty() {
            errors.push("Entry function has no basic blocks".to_string());
        }

        // Enforce required attributes
        for attr in ENTRY_ATTRIBUTE_KEYS.iter().copied().filter(|attr| {
            *attr != "entry_point"
                && !(*attr == "required_num_qubits" && capability_flags.dynamic_qubit_management)
                && !(*attr == "required_num_results" && capability_flags.dynamic_result_management)
        }) {
            let val = entry_fn.get_string_attribute(AttributeLoc::Function, attr);
            if val.is_none() {
                errors.push(format!("Missing required attribute: `{attr}`"));
            }
        }

        // `required_num_qubits` must stay positive. `required_num_results`
        // may be zero for programs that only use classical-returning operations
        // such as `mz_leaked`.
        for (attr, type_) in [("required_num_qubits", "qubit")] {
            if capability_flags.dynamic_qubit_management {
                continue;
            }
            if entry_fn
                .get_string_attribute(AttributeLoc::Function, attr)
                .and_then(|a| {
                    decode_llvm_c_string(a.get_string_value())?
                        .parse::<u32>()
                        .ok()
                })
                == Some(0)
            {
                errors.push(format!("Entry function must have at least one {type_}"));
            }
        }
        entry_fn
    } else {
        errors.push("No entry function found in QIR module".to_string());
        return Err(errors.join("; "));
    };

    let wasm_fns = get_wasm_functions(wasm_bytes)?;

    validate_functions(&module, entry_fn, &wasm_fns, &mut errors);
    validate_result_slot_usage(&module, entry_fn, &mut errors);
    validate_dynamic_result_allocation_placement(&module, entry_fn, &mut errors);
    validate_dynamic_array_allocation_backing(&module, &mut errors);

    validate_module_flags(&module, &mut errors);
    validate_capability_usage(&module, capability_flags, &mut errors);

    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    log::info!("QIR validation passed");
    Ok(())
}

/// Convert QIR LLVM IR text to QIR bitcode bytes.
///
/// # Errors
/// Returns an error string if the LLVM IR is invalid.
pub fn qir_ll_to_bc(ll_text: &str) -> Result<Vec<u8>, String> {
    use inkwell::context::Context;

    let ctx = Context::create();
    let module = create_module_from_ir_text(&ctx, ll_text, "qir")?;

    Ok(memory_buffer_to_owned_bytes(
        &module.write_bitcode_to_memory(),
    ))
}

fn decode_string_attribute_kind(attr: inkwell::attributes::Attribute) -> Result<String, String> {
    use llvm_sys::core::LLVMGetStringAttributeKind;
    use std::slice;

    let mut kind_len = 0_u32;
    let kind_ptr = unsafe { LLVMGetStringAttributeKind(attr.as_mut_ptr(), &raw mut kind_len) };
    if kind_ptr.is_null() {
        return Err("LLVM returned a null attribute kind pointer".to_string());
    }
    let kind_len = usize::try_from(kind_len)
        .map_err(|_| "Attribute kind length does not fit into usize".to_string())?;
    let kind_bytes = unsafe { slice::from_raw_parts(kind_ptr.cast::<u8>(), kind_len) };
    std::str::from_utf8(kind_bytes)
        .map_err(|e| format!("Invalid UTF-8 in attribute kind: {e}"))
        .map(str::to_owned)
}

fn decode_string_attribute_value(
    attr: inkwell::attributes::Attribute,
    kind: &str,
) -> Result<Option<String>, String> {
    use llvm_sys::core::LLVMGetStringAttributeValue;
    use std::slice;

    let mut value_len = 0_u32;
    let value_ptr = unsafe { LLVMGetStringAttributeValue(attr.as_mut_ptr(), &raw mut value_len) };
    if value_len == 0 {
        return Ok(None);
    }
    if value_ptr.is_null() {
        return Err(format!(
            "LLVM returned a null attribute value pointer for `{kind}`"
        ));
    }
    let value_len = usize::try_from(value_len)
        .map_err(|_| format!("Attribute `{kind}` value length does not fit into usize"))?;
    let value_bytes = unsafe { slice::from_raw_parts(value_ptr.cast::<u8>(), value_len) };
    let value = std::str::from_utf8(value_bytes)
        .map_err(|e| format!("Invalid UTF-8 in attribute `{kind}` value: {e}"))?
        .to_owned();
    Ok(Some(value))
}

/// Get QIR entry point function attributes.
///
/// These attributes are used to generate METADATA records in QIR output schemas.
/// This function assumes that QIR has been validated using `validate_qir`.
///
/// # Errors
/// Returns an error string if the input bitcode is invalid.
pub fn get_entry_attributes(
    bc_bytes: &[u8],
) -> Result<std::collections::BTreeMap<String, Option<String>>, String> {
    use crate::convert::{find_entry_function, get_string_attrs};
    use inkwell::context::Context;
    use std::collections::BTreeMap;

    let ctx = Context::create();
    let module = parse_bitcode_module(&ctx, bc_bytes, "bitcode")?;

    let mut metadata = BTreeMap::new();
    if let Ok(entry_fn) = find_entry_function(&module) {
        for attr in get_string_attrs(entry_fn) {
            let kind_id = match decode_string_attribute_kind(attr) {
                Ok(kind_id) => kind_id,
                Err(err) => {
                    log::warn!("Skipping attribute with invalid kind: {err}");
                    continue;
                }
            };
            match decode_string_attribute_value(attr, &kind_id) {
                Ok(value) => {
                    metadata.insert(kind_id, value);
                }
                Err(err) => {
                    log::warn!("{err}");
                    metadata.insert(kind_id, None);
                }
            }
        }
    }
    Ok(metadata)
}

#[cfg(feature = "python")]
mod exceptions {
    use pyo3::exceptions::PyException;
    use pyo3_stub_gen::create_exception;

    create_exception!(
        qir_qis,
        ValidationError,
        PyException,
        "QIR ValidationError.\n\nRaised when the QIR is invalid."
    );
    create_exception!(
        qir_qis,
        CompilerError,
        PyException,
        "QIR CompilerError.\n\nRaised when QIR to QIS compilation fails."
    );
}

#[cfg(feature = "python")]
#[pymodule]
mod qir_qis {
    use std::borrow::Cow;
    use std::collections::BTreeMap;

    use super::{PyErr, PyResult, pyfunction};

    use pyo3_stub_gen::derive::gen_stub_pyfunction;

    #[pymodule_export]
    use super::exceptions::CompilerError;
    #[pymodule_export]
    use super::exceptions::ValidationError;

    /// Validate the given QIR.
    ///
    /// # Arguments
    /// - `bc_bytes` - The QIR bytes to validate.
    /// - `wasm_bytes` - Optional WASM bytes to validate against.
    ///
    /// # Errors
    /// Returns a `ValidationError`:
    /// - If the QIR is invalid.
    /// - If the WASM module is invalid.
    /// - If a QIR-referenced WASM function is missing from the WASM module.
    #[gen_stub_pyfunction]
    #[pyfunction]
    #[allow(clippy::needless_pass_by_value)]
    #[pyo3(signature = (bc_bytes, *, wasm_bytes = None))]
    pub fn validate_qir(bc_bytes: Cow<[u8]>, wasm_bytes: Option<Cow<[u8]>>) -> PyResult<()> {
        crate::validate_qir(&bc_bytes, wasm_bytes.as_deref())
            .map_err(PyErr::new::<ValidationError, _>)
    }

    /// Translate QIR bitcode to Quantinuum QIS.
    ///
    /// # Arguments
    /// - `bc_bytes` - The QIR bytes to translate.
    /// - `opt_level` - The optimization level to use (0-3). Default is 2 on
    ///   Linux/macOS and 0 on Windows.
    /// - `target` - Target architecture (default: "aarch64" on Linux/macOS and
    ///   "native" on Windows; options: "x86-64", "native").
    /// - `wasm_bytes` - Optional WASM bytes for Wasm codegen.
    ///
    /// # Errors
    /// Returns a `CompilerError` if the translation fails.
    #[gen_stub_pyfunction]
    #[pyfunction]
    #[allow(clippy::needless_pass_by_value)]
    #[allow(clippy::missing_errors_doc)]
    #[cfg_attr(
        windows,
        pyo3(signature = (bc_bytes, *, opt_level = 0, target = "native", wasm_bytes = None))
    )]
    #[cfg_attr(
        not(windows),
        pyo3(signature = (bc_bytes, *, opt_level = 2, target = "aarch64", wasm_bytes = None))
    )]
    pub fn qir_to_qis<'a>(
        bc_bytes: Cow<[u8]>,
        opt_level: u32,
        target: &'a str,
        wasm_bytes: Option<Cow<'a, [u8]>>,
    ) -> PyResult<Cow<'a, [u8]>> {
        let result = crate::qir_to_qis(&bc_bytes, opt_level, target, wasm_bytes.as_deref())
            .map_err(PyErr::new::<CompilerError, _>)?;

        Ok(result.into())
    }

    /// Convert QIR LLVM IR to QIR bitcode.
    ///
    /// # Errors
    /// Returns a `ValidationError` if the LLVM IR is invalid.
    #[gen_stub_pyfunction]
    #[pyfunction]
    fn qir_ll_to_bc(ll_text: &str) -> PyResult<Cow<'_, [u8]>> {
        let result = crate::qir_ll_to_bc(ll_text).map_err(PyErr::new::<ValidationError, _>)?;
        Ok(result.into())
    }

    /// Get QIR entry point function attributes.
    ///
    /// These attributes are used to generate METADATA records in QIR output schemas.
    /// This function assumes that QIR has been validated using `validate_qir`.
    ///
    /// # Errors
    /// Returns a `ValidationError` if the input bitcode is invalid.
    #[gen_stub_pyfunction]
    #[pyfunction]
    #[allow(clippy::needless_pass_by_value)]
    fn get_entry_attributes(bc_bytes: Cow<[u8]>) -> PyResult<BTreeMap<String, Option<String>>> {
        crate::get_entry_attributes(&bc_bytes).map_err(PyErr::new::<ValidationError, _>)
    }
}

#[cfg(feature = "python")]
define_stub_info_gatherer!(stub_info);

#[cfg(test)]
mod test {
    #![allow(clippy::expect_used)]
    #![allow(clippy::unwrap_used)]
    use crate::{
        convert::get_string_label, create_module_from_ir_text, get_entry_attributes,
        parse_bitcode_module, qir_ll_to_bc, qir_to_qis, validate_qir,
    };
    use inkwell::{
        context::Context,
        memory_buffer::MemoryBuffer,
        module::Module,
        values::{CallSiteValue, FunctionValue},
    };
    use proptest::prelude::*;
    use std::{collections::BTreeMap, sync::LazyLock};
    #[cfg(feature = "wasm")]
    use wasm_encoder::{ExportKind, ExportSection, Module as WasmModule};

    const PROPERTY_FIXTURES: &[&str] = &[
        "tests/data/base.ll",
        "tests/data/base_array.ll",
        "tests/data/adaptive.ll",
        "tests/data/qir2_base.ll",
        "tests/data/qir2_adaptive.ll",
        "tests/data/mz_leaked.ll",
    ];
    const DYNAMIC_FEATURE_FIXTURES: &[&str] = &[
        "tests/data/dynamic_qubit_alloc.ll",
        "tests/data/dynamic_qubit_alloc_checked.ll",
        "tests/data/dynamic_qubit_array_checked.ll",
        "tests/data/dynamic_qubit_array_ssa.ll",
        "tests/data/dynamic_result_alloc.ll",
        "tests/data/dynamic_result_mixed_array_output.ll",
    ];
    static PROPERTY_FIXTURE_BITCODE: LazyLock<BTreeMap<&'static str, Vec<u8>>> =
        LazyLock::new(|| {
            PROPERTY_FIXTURES
                .iter()
                .map(|path| {
                    let ll_text =
                        std::fs::read_to_string(path).expect("Failed to read LLVM IR fixture");
                    let bitcode = qir_ll_to_bc(&ll_text)
                        .expect("Failed to convert LLVM IR fixture to bitcode");
                    (*path, bitcode)
                })
                .collect()
        });

    fn conservative_translation_settings() -> (u32, &'static str) {
        (0, "native")
    }

    fn load_fixture_bitcode(path: &str) -> Vec<u8> {
        PROPERTY_FIXTURE_BITCODE
            .get(path)
            .cloned()
            .expect("Fixture bitcode should be precompiled")
    }

    fn verify_bitcode_module(bitcode: &[u8], name: &str) -> Result<(), String> {
        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, bitcode, name)?;
        crate::llvm_verify::verify_module(&module, "LLVM verifier rejected translated module")
    }

    fn parse_bitcode_as_file(bitcode: &[u8], name: &str) -> Result<(), String> {
        let mut temp_file = tempfile::Builder::new()
            .prefix(name)
            .suffix(".bc")
            .tempfile()
            .map_err(|e| format!("Failed to create temp bitcode file: {e}"))?;
        std::io::Write::write_all(&mut temp_file, bitcode)
            .map_err(|e| format!("Failed to write temp bitcode: {e}"))?;

        let ctx = Context::create();
        let memory_buffer = MemoryBuffer::create_from_file(temp_file.path())
            .map_err(|e| format!("Failed to read temp bitcode: {e}"))?;
        Module::parse_bitcode_from_buffer(&memory_buffer, &ctx)
            .map(|_| ())
            .map_err(|e| format!("Failed to parse bitcode: {e}"))
    }

    fn assert_public_bitcode_round_trips_from_file(bitcode: &[u8], name: &str) {
        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, bitcode, name)
            .expect("Bitcode should reparse through qir-qis helpers");
        let raw_buffer = module.write_bitcode_to_memory();
        let expected_len = bitcode
            .len()
            .checked_add(1)
            .expect("bitcode length should not overflow");
        assert_eq!(
            raw_buffer.as_slice().len(),
            expected_len,
            "Public bitcode bytes should exclude LLVM's implicit trailing NUL"
        );
        assert_eq!(raw_buffer.as_slice().last(), Some(&0));
        parse_bitcode_as_file(bitcode, name)
            .expect("Public bitcode should parse when consumed from a file");
    }

    #[cfg(feature = "wasm")]
    fn build_wasm_exports(exports: &[(String, u32)]) -> Vec<u8> {
        let mut module = WasmModule::new();
        let mut export_section = ExportSection::new();
        for (name, index) in exports {
            export_section.export(name, ExportKind::Func, *index);
        }
        module.section(&export_section);
        module.finish()
    }

    fn minimal_qir_with_body(
        required_num_qubits: &str,
        required_num_results: &str,
        qir_major_flag: &str,
        extra_decl: &str,
        body: &str,
    ) -> String {
        format!(
            r#"%Qubit = type opaque
%Result = type opaque

{extra_decl}

define i64 @Entry_Point_Name() #0 {{
entry:
{body}
  ret i64 0
}}

attributes #0 = {{ "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="{required_num_qubits}" "required_num_results"="{required_num_results}" }}

!llvm.module.flags = !{{!0, !1, !2, !3}}
!0 = !{{i32 1, !"qir_major_version", i32 {qir_major_flag}}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
!2 = !{{i32 1, !"dynamic_qubit_management", i1 false}}
!3 = !{{i32 1, !"dynamic_result_management", i1 false}}
"#
        )
    }

    fn minimal_qir_missing_attr(missing_attr: &str) -> String {
        let attrs = [
            ("entry_point", None),
            ("qir_profiles", Some("base_profile")),
            ("output_labeling_schema", Some("schema_id")),
            ("required_num_qubits", Some("1")),
            ("required_num_results", Some("1")),
        ];
        let rendered_attrs = attrs
            .into_iter()
            .filter(|(name, _)| *name != missing_attr)
            .map(|(name, value)| {
                value.map_or_else(
                    || format!(r#""{name}""#),
                    |value| format!(r#""{name}"="{value}""#),
                )
            })
            .collect::<Vec<_>>()
            .join(" ");

        format!(
            r#"
define i64 @Entry_Point_Name() #0 {{
entry:
  ret i64 0
}}

attributes #0 = {{ {rendered_attrs} }}

!llvm.module.flags = !{{!0, !1, !2, !3}}
!0 = !{{i32 1, !"qir_major_version", i32 1}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
!2 = !{{i32 1, !"dynamic_qubit_management", i1 false}}
!3 = !{{i32 1, !"dynamic_result_management", i1 false}}
"#
        )
    }

    fn minimal_qir_with_duplicate_major_flags(first_major: &str, second_major: &str) -> String {
        format!(
            r#"
define i64 @Entry_Point_Name() #0 {{
entry:
  ret i64 0
}}

attributes #0 = {{ "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }}

!llvm.module.flags = !{{!0, !1, !2, !3, !4}}
!0 = !{{i32 1, !"qir_major_version", i32 {first_major}}}
!1 = !{{i32 1, !"qir_major_version", i32 {second_major}}}
!2 = !{{i32 7, !"qir_minor_version", i32 0}}
!3 = !{{i32 1, !"dynamic_qubit_management", i1 false}}
!4 = !{{i32 1, !"dynamic_result_management", i1 false}}
"#
        )
    }

    fn minimal_qir_with_duplicate_dynamic_flags(first_flag: &str, second_flag: &str) -> String {
        format!(
            r#"
define i64 @Entry_Point_Name() #0 {{
entry:
  ret i64 0
}}

attributes #0 = {{ "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }}

!llvm.module.flags = !{{!0, !1, !2, !3, !4}}
!0 = !{{i32 1, !"qir_major_version", i32 1}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
!2 = !{{i32 1, !"dynamic_qubit_management", i1 {first_flag}}}
!3 = !{{i32 1, !"dynamic_qubit_management", i1 {second_flag}}}
!4 = !{{i32 1, !"dynamic_result_management", i1 false}}
"#
        )
    }

    fn collect_called_function_names(helper: FunctionValue<'_>) -> Vec<String> {
        let mut calls = Vec::new();
        for bb in helper.get_basic_blocks() {
            for instr in bb.get_instructions() {
                let Ok(call) = CallSiteValue::try_from(instr) else {
                    continue;
                };
                let Some(callee) = call.get_called_fn_value() else {
                    continue;
                };
                let Ok(name) = callee.get_name().to_str() else {
                    continue;
                };
                calls.push(name.to_string());
            }
        }
        calls
    }

    #[test]
    fn test_get_entry_attributes() {
        let ll_text = std::fs::read_to_string("tests/data/base-attrs.ll")
            .expect("Failed to read base-attrs.ll");
        let bc_bytes = qir_ll_to_bc(&ll_text).unwrap();
        let attrs = get_entry_attributes(&bc_bytes).unwrap();
        assert!(matches!(attrs.get("entry_point"), Some(None)));
        assert_eq!(
            attrs.get("qir_profiles"),
            Some(&Some("base_profile".to_string()))
        );
        assert_eq!(
            attrs.get("output_labeling_schema"),
            Some(&Some("labeled".to_string()))
        );
        assert_eq!(
            attrs.get("required_num_qubits"),
            Some(&Some("2".to_string()))
        );
        assert_eq!(
            attrs.get("required_num_results"),
            Some(&Some("2".to_string()))
        );
    }

    #[test]
    fn test_entry_attributes_includes_optional_custom_attr() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="labeled" "required_num_qubits"="2" "required_num_results"="2" "custom_attr"="custom_value" }

!llvm.module.flags = !{!0, !1, !2, !3}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let attrs = get_entry_attributes(&bc_bytes).unwrap();
        assert_eq!(
            attrs.get("custom_attr"),
            Some(&Some("custom_value".to_string()))
        );
    }

    #[test]
    fn test_get_entry_attributes_is_order_independent() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "custom_attr"="custom_value" "required_num_results"="2" "output_labeling_schema"="labeled" "entry_point" "required_num_qubits"="2" "qir_profiles"="base_profile" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let attrs = get_entry_attributes(&bc_bytes).expect("entry attributes should parse");
        assert!(matches!(attrs.get("entry_point"), Some(None)));
        assert_eq!(
            attrs.get("qir_profiles"),
            Some(&Some("base_profile".to_string()))
        );
        assert_eq!(
            attrs.get("custom_attr"),
            Some(&Some("custom_value".to_string()))
        );
    }

    #[test]
    fn test_qir_to_qis_strips_custom_entry_attrs() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="labeled" "required_num_qubits"="2" "required_num_results"="2" "custom_attr"="custom_value" }

!llvm.module.flags = !{!0, !1, !2, !3}

!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let (opt_level, target) = if cfg!(windows) {
            (0, "native")
        } else {
            (2, "aarch64")
        };
        let qis_bytes = qir_to_qis(&bc_bytes, opt_level, target, None).unwrap();

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &qis_bytes, "qis").unwrap();
        let entry_fn = module.get_function("___user_qir_Entry_Point_Name").unwrap();

        assert!(
            entry_fn
                .get_string_attribute(inkwell::attributes::AttributeLoc::Function, "custom_attr")
                .is_none()
        );
    }

    #[test]
    fn test_platform_default_conversion_settings_match_expectations() {
        if cfg!(windows) {
            assert_eq!(crate::DEFAULT_OPT_LEVEL, 0);
            assert_eq!(crate::DEFAULT_TARGET, "native");
        } else {
            assert_eq!(crate::DEFAULT_OPT_LEVEL, 2);
            assert_eq!(crate::DEFAULT_TARGET, "aarch64");
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_windows_optimized_conversion_returns_actionable_error() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="labeled" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = qir_to_qis(&bc_bytes, 1, "native", None)
            .expect_err("optimized conversion should fail fast on Windows");
        assert!(err.contains("currently unavailable on Windows"));
        assert!(err.contains("opt_level=0"));
    }

    #[test]
    fn test_qir_ll_to_bc_accepts_legacy_typed_pointers() {
        let ll_text =
            std::fs::read_to_string("tests/data/base.ll").expect("Failed to read base.ll");
        let bc_bytes = qir_ll_to_bc(&ll_text).unwrap();
        assert!(!bc_bytes.is_empty());
    }

    #[test]
    fn test_qir_ll_to_bc_output_parses_when_read_from_file() {
        let ll_text =
            std::fs::read_to_string("tests/data/base.ll").expect("Failed to read base.ll");
        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert base.ll to bitcode");

        assert_public_bitcode_round_trips_from_file(&bc_bytes, "public_qir_output");
    }

    #[test]
    fn test_qir2_base_fixture_validate_and_compile() {
        let ll_text = std::fs::read_to_string("tests/data/qir2_base.ll")
            .expect("Failed to read qir2_base.ll");
        let input_bc = qir_ll_to_bc(&ll_text).expect("Failed to convert qir2_base.ll to bitcode");

        validate_qir(&input_bc, None).expect("QIR 2.0 base fixture should validate");
        let output_bc =
            qir_to_qis(&input_bc, 0, "native", None).expect("QIR 2.0 base fixture should compile");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "qis_module")
            .expect("Compiled QIS bitcode should parse");
        assert!(module.get_function("qmain").is_some());
        assert!(module.get_function("qir_qis.load_qubit").is_some());
    }

    #[test]
    fn test_qir2_adaptive_fixture_validate_and_compile() {
        let ll_text = std::fs::read_to_string("tests/data/qir2_adaptive.ll")
            .expect("Failed to read qir2_adaptive.ll");
        let input_bc =
            qir_ll_to_bc(&ll_text).expect("Failed to convert qir2_adaptive.ll to bitcode");

        validate_qir(&input_bc, None).expect("QIR 2.0 adaptive fixture should validate");
        let output_bc = qir_to_qis(&input_bc, 0, "native", None)
            .expect("QIR 2.0 adaptive fixture should compile");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "qis_module")
            .expect("Compiled QIS bitcode should parse");
        assert!(module.get_function("qmain").is_some());
        assert!(module.get_function("___lazy_measure").is_some());
    }

    #[test]
    fn test_qir_to_qis_output_parses_with_raw_llvm_buffer() {
        let ll_text =
            std::fs::read_to_string("tests/data/base.ll").expect("Failed to read base.ll");
        let input_bc = qir_ll_to_bc(&ll_text).expect("Failed to convert base.ll to bitcode");
        let output_bc =
            qir_to_qis(&input_bc, 0, "native", None).expect("base fixture should compile");

        assert_public_bitcode_round_trips_from_file(&output_bc, "selene_qis_output");
    }

    #[test]
    fn test_validate_module_flags_are_checked_cross_platform() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        validate_qir(&bc_bytes, None).expect("Module flags should validate on every platform");
    }

    #[test]
    fn test_module_flag_parser_reads_existing_flags() {
        use crate::aux::collect_module_flags;
        use inkwell::context::Context;

        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let ctx = Context::create();
        let module = create_module_from_ir_text(&ctx, ll_text, "qir")
            .expect("Failed to create module from inline IR");

        let flags = collect_module_flags(&module);
        assert_eq!(
            flags.get("qir_major_version").map(<[String]>::to_vec),
            Some(vec!["i32 2".to_string()])
        );
        assert_eq!(
            flags.get("qir_minor_version").map(<[String]>::to_vec),
            Some(vec!["i32 0".to_string()])
        );
        assert_eq!(
            flags
                .get("dynamic_qubit_management")
                .map(<[String]>::to_vec),
            Some(vec!["i1 false".to_string()])
        );
        assert_eq!(
            flags
                .get("dynamic_result_management")
                .map(<[String]>::to_vec),
            Some(vec!["i1 false".to_string()])
        );
    }

    #[test]
    fn test_validate_module_flags_accept_duplicate_entries_if_one_matches() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 99}
!1 = !{i32 1, !"qir_major_version", i32 2}
!2 = !{i32 7, !"qir_minor_version", i32 0}
!3 = !{i32 1, !"dynamic_qubit_management", i1 false}
!4 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        validate_qir(&bc_bytes, None)
            .expect("Module flags should validate when any duplicate entry matches");
    }

    #[test]
    fn test_validate_module_flags_reports_malformed_required_flag() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", !4}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 99}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None).expect_err("Malformed module flag should fail");
        assert!(err.contains("Missing or unsupported module flag: qir_major_version"));
    }

    #[test]
    fn test_validate_qir_reports_exact_single_expected_module_flag_value() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 99}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("unsupported single-valued module flag should fail");
        assert!(err.contains("Unsupported qir_minor_version: expected i32 0"));
    }

    #[test]
    fn test_validate_qir_reports_unsupported_optional_arrays_module_flag_value() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i32 7}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("unsupported optional arrays flag should fail validation");
        assert!(err.contains("Unsupported arrays: expected one of i1 false, i1 true"));
    }

    #[test]
    fn test_validate_qir_reports_malformed_optional_arrays_module_flag() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", !5}
!5 = !{i32 99}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err =
            validate_qir(&bc_bytes, None).expect_err("malformed optional arrays flag should fail");
        assert!(err.contains("Missing or unsupported module flag: arrays"));
    }

    #[test]
    fn test_validate_qir_missing_required_module_flag_reports_exact_message() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2}
!0 = !{i32 7, !"qir_minor_version", i32 0}
!1 = !{i32 1, !"dynamic_qubit_management", i1 false}
!2 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None).expect_err("Missing flag should fail");
        assert!(err.contains("Missing required module flag: qir_major_version"));
    }

    #[test]
    fn test_qir_to_qis_bool_output_uses_bool_tag_and_print_bool() {
        let ll_text = r#"
%Result = type opaque

@bool_out = private constant [2 x i8] c"b\00"

declare void @__quantum__rt__bool_record_output(i1, ptr)

define i64 @Entry_Point_Name() #0 {
entry:
  call void @__quantum__rt__bool_record_output(i1 true, ptr getelementptr inbounds ([2 x i8], ptr @bool_out, i64 0, i64 0))
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let output_bc =
            qir_to_qis(&bc_bytes, 0, "native", None).expect("bool output should compile");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "qis_module")
            .expect("Compiled QIS bitcode should parse");
        assert!(module.get_function("print_bool").is_some());

        #[cfg(not(windows))]
        {
            let text = module.to_string();
            assert!(text.contains("USER:BOOL:b"));
        }

        #[cfg(windows)]
        {
            let labels = module
                .get_globals()
                .filter_map(|global| crate::convert::get_string_label(global).ok())
                .collect::<Vec<_>>();
            assert!(labels.iter().any(|label| label.contains("USER:BOOL:b")));
        }
    }

    #[test]
    fn test_validate_qir_rejects_malformed_barrier_suffix() {
        let ll_text = r#"
%Qubit = type opaque

declare void @__quantum__qis__barrier2__adj(%Qubit*, %Qubit*)

define i64 @Entry_Point_Name() #0 {
entry:
  %q0 = inttoptr i64 1 to %Qubit*
  %q1 = inttoptr i64 2 to %Qubit*
  call void @__quantum__qis__barrier2__adj(%Qubit* %q0, %Qubit* %q1)
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="2" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None).expect_err("malformed barrier suffix should fail");
        assert!(err.contains("Unsupported QIR QIS function: __quantum__qis__barrier2__adj"));
    }

    #[test]
    fn test_validate_qir_accepts_barrier_matching_required_qubits() {
        let ll_text = r#"
%Qubit = type opaque

declare void @__quantum__qis__barrier2__body(%Qubit*, %Qubit*)

define i64 @Entry_Point_Name() #0 {
entry:
  %q0 = inttoptr i64 1 to %Qubit*
  %q1 = inttoptr i64 2 to %Qubit*
  call void @__quantum__qis__barrier2__body(%Qubit* %q0, %Qubit* %q1)
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="2" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        validate_qir(&bc_bytes, None)
            .expect("barrier arity matching required_num_qubits should validate");
    }

    #[test]
    fn test_validate_qir_rejects_zero_arity_barrier() {
        let ll_text = r#"
%Qubit = type opaque

declare void @__quantum__qis__barrier0__body()

define i64 @Entry_Point_Name() #0 {
entry:
  call void @__quantum__qis__barrier0__body()
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None).expect_err("barrier0 should be rejected");
        assert!(err.contains("Unsupported QIR QIS function: __quantum__qis__barrier0__body"));
    }

    #[test]
    fn test_validate_qir_rejects_unsupported_qtm_function() {
        let ll_text = r#"
declare void @___unknown_qtm()

define i64 @Entry_Point_Name() #0 {
entry:
  call void @___unknown_qtm()
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("unsupported QTM declarations should fail validation");
        assert!(err.contains("Unsupported Qtm QIS function: ___unknown_qtm"));
    }

    #[test]
    fn test_validate_qir_allows_ir_defined_non_main_helper() {
        let ll_text = r#"
%Qubit = type opaque

define void @helper(%Qubit* %qubit) {
entry:
  call void @__quantum__qis__h__body(%Qubit* %qubit)
  ret void
}

define i64 @Entry_Point_Name() #0 {
entry:
  %q0 = inttoptr i64 1 to %Qubit*
  call void @helper(%Qubit* %q0)
  ret i64 0
}

declare void @__quantum__qis__h__body(%Qubit*)

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        validate_qir(&bc_bytes, None)
            .expect("IR-defined helper functions with non-main names should be allowed");
    }

    #[test]
    fn test_validate_qir_allows_external_pointer_returning_declarations() {
        let ll_text = r#"
%Qubit = type opaque

declare ptr @external_helper()

define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        validate_qir(&bc_bytes, None)
            .expect("external declarations without bodies should not be treated as IR-defined");
    }

    #[test]
    fn test_validate_qir_rejects_ir_defined_pointer_returning_function() {
        let ll_text = r#"
define ptr @helper() {
entry:
  ret ptr null
}

define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("IR-defined pointer-returning helper should fail validation");
        assert!(err.contains("Function `helper` cannot return a pointer type"));
    }

    #[test]
    fn test_qir_to_qis_rejects_unknown_declared_qis_function() {
        let ll_text = r#"
%Qubit = type opaque

declare void @__quantum__qis__mystery__body(%Qubit*)

define i64 @Entry_Point_Name() #0 {
entry:
  %q0 = inttoptr i64 1 to %Qubit*
  call void @__quantum__qis__mystery__body(%Qubit* %q0)
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect_err("unknown declared QIS function should fail");
        assert!(err.contains("Unsupported QIR QIS function: __quantum__qis__mystery__body"));
    }

    #[test]
    fn test_qir_to_qis_u1q_synonym_lowers_to_rxy() {
        let ll_text = r#"
%Qubit = type opaque

declare void @__quantum__qis__u1q__body(double, double, %Qubit*)

define i64 @Entry_Point_Name() #0 {
entry:
  %q0 = inttoptr i64 1 to %Qubit*
  call void @__quantum__qis__u1q__body(double 1.0, double 0.5, %Qubit* %q0)
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let output_bc =
            qir_to_qis(&bc_bytes, 0, "native", None).expect("u1q synonym should compile");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "qis_module")
            .expect("Compiled QIS bitcode should parse");
        assert!(module.get_function("___rxy").is_some());

        #[cfg(not(windows))]
        {
            let text = module.to_string();
            assert!(text.contains("___rxy"));
        }
    }

    #[test]
    fn test_checked_result_index_rejects_out_of_bounds_values() {
        let err = crate::aux::checked_result_index(5, 1)
            .expect_err("out-of-bounds result indices should fail cleanly");
        assert_eq!(err, "Result index 5 exceeds required_num_results (1)");
    }

    #[test]
    fn test_validate_qir_allows_zero_required_num_results_for_mz_leaked() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            r#"
declare i64 @__quantum__qis__mz_leaked__body(%Qubit*)
declare void @__quantum__rt__int_record_output(i64, i8*)

@0 = private constant [7 x i8] c"leaked\00"
"#,
            r"  %q0 = inttoptr i64 0 to %Qubit*
  %0 = call i64 @__quantum__qis__mz_leaked__body(%Qubit* %q0)
  call void @__quantum__rt__int_record_output(i64 %0, i8* getelementptr inbounds ([7 x i8], [7 x i8]* @0, i64 0, i64 0))",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        validate_qir(&bc_bytes, None)
            .expect("mz_leaked-only programs should validate with zero result slots");
    }

    #[test]
    fn test_validate_qir_reports_invalid_required_num_results_value() {
        let ll_text = minimal_qir_with_body("1", "abc", "1", "", "");

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("invalid required_num_results should fail validation");
        assert!(err.contains("Invalid required_num_results attribute value: abc"));
    }

    #[test]
    fn test_validate_qir_reports_missing_required_num_results_once() {
        let ll_text = r#"
%Qubit = type opaque

define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err =
            validate_qir(&bc_bytes, None).expect_err("missing required_num_results should fail");
        assert_eq!(err, "Missing required attribute: `required_num_results`");
    }

    #[test]
    fn test_validate_qir_rejects_result_usage_in_ir_defined_helper_with_zero_slots() {
        let ll_text = r#"
%Qubit = type opaque
%Result = type opaque

declare i1 @__quantum__rt__read_result(%Result*)

define internal void @helper() {
entry:
  %0 = call i1 @__quantum__rt__read_result(%Result* null)
  ret void
}

define i64 @Entry_Point_Name() #0 {
entry:
  call void @helper()
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="0" }

!llvm.module.flags = !{!0, !1, !2, !3}
!0 = !{i32 1, !"qir_major_version", i32 1}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None).expect_err(
            "result usage in IR-defined helpers should still respect required_num_results",
        );
        assert!(err.contains("Result index 0 exceeds required_num_results (0)"));
    }

    #[test]
    fn test_validate_qir_rejects_zero_required_num_results_for_result_measurement() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            "declare void @__quantum__qis__mz__body(%Qubit*, %Result* writeonly)",
            r"  call void @__quantum__qis__mz__body(%Qubit* null, %Result* writeonly null)",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("result-backed measurements should fail validation without result slots");
        assert!(err.contains("Result index 0 exceeds required_num_results (0)"));
    }

    #[test]
    fn test_validate_qir_rejects_zero_required_num_results_for_read_result() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            "declare i1 @__quantum__rt__read_result(%Result*)",
            r"  %0 = call i1 @__quantum__rt__read_result(%Result* null)",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("result reads should fail validation without result slots");
        assert!(err.contains("Result index 0 exceeds required_num_results (0)"));
    }

    #[test]
    fn test_validate_qir_rejects_zero_required_num_results_for_result_record_output() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            r#"
declare void @__quantum__rt__result_record_output(%Result*, i8*)

@0 = private constant [4 x i8] c"res\00"
"#,
            r"  call void @__quantum__rt__result_record_output(%Result* null, i8* getelementptr inbounds ([4 x i8], [4 x i8]* @0, i64 0, i64 0))",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("result output should fail validation without result slots");
        assert!(err.contains("Result index 0 exceeds required_num_results (0)"));
    }

    #[test]
    fn test_validate_qir_rejects_out_of_bounds_result_measurement_index() {
        let ll_text = minimal_qir_with_body(
            "1",
            "1",
            "1",
            "declare void @__quantum__qis__mz__body(%Qubit*, %Result* writeonly)",
            r"  call void @__quantum__qis__mz__body(%Qubit* null, %Result* writeonly inttoptr (i64 5 to %Result*))",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = validate_qir(&bc_bytes, None)
            .expect_err("out-of-bounds result indices should fail during validation");
        assert!(err.contains("Result index 5 exceeds required_num_results (1)"));
    }

    #[test]
    fn test_qir_to_qis_rejects_malformed_mz_leaked_call() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            "declare i64 @__quantum__qis__mz_leaked__body()",
            r"  %0 = call i64 @__quantum__qis__mz_leaked__body()",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect_err("malformed mz_leaked calls should fail cleanly");
        assert!(err.contains("Malformed mz_leaked call"));
    }

    #[test]
    fn test_qir_to_qis_rejects_mz_leaked_with_wrong_return_type() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            "declare void @__quantum__qis__mz_leaked__body(%Qubit*)",
            r"  call void @__quantum__qis__mz_leaked__body(%Qubit* null)",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect_err("mz_leaked with the wrong signature should fail cleanly");
        assert!(err.contains("Malformed mz_leaked call: expected signature i64 (ptr)"));
    }

    #[test]
    fn test_qir_to_qis_rejects_mz_leaked_with_wrong_return_width() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            "declare i1 @__quantum__qis__mz_leaked__body(%Qubit*)",
            r"  %0 = call i1 @__quantum__qis__mz_leaked__body(%Qubit* null)",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect_err("mz_leaked with the wrong return width should fail cleanly");
        assert_eq!(
            err,
            "Malformed mz_leaked call: expected signature i64 (ptr)"
        );
    }

    #[test]
    fn test_qir_to_qis_rejects_mz_leaked_with_non_pointer_parameter() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            "declare i64 @__quantum__qis__mz_leaked__body(i64)",
            r"  %0 = call i64 @__quantum__qis__mz_leaked__body(i64 0)",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect_err("mz_leaked with a non-pointer parameter should fail cleanly");
        assert_eq!(
            err,
            "Malformed mz_leaked call: expected signature i64 (ptr)"
        );
    }

    #[test]
    fn test_mz_leaked_operand_check_rejects_non_pointer_first_operand() {
        let ctx = Context::create();
        let value = ctx.i64_type().const_zero().into();
        let err = crate::aux::mz_leaked_qubit_operand(&[value, value])
            .expect_err("non-pointer mz_leaked operands should fail cleanly");
        assert_eq!(
            err,
            "Malformed mz_leaked call: expected first argument to be a pointer"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_qir_to_qis_mz_leaked_lowers_via_uint_future_runtime() {
        let bc_bytes = load_fixture_bitcode("tests/data/mz_leaked.ll");
        let output_bc = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect("mz_leaked fixture should compile successfully");

        verify_bitcode_module(&output_bc, "mz_leaked_qis")
            .expect("translated leaked-measure module should remain LLVM-verifiable");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "mz_leaked_qis")
            .expect("Compiled QIS bitcode should parse");
        let text = module.to_string();
        assert!(text.contains("___lazy_measure_leaked"));
        assert!(text.contains("___read_future_uint"));
        assert!(text.contains("___dec_future_refcount"));
        assert!(!text.contains("___read_future_bool"));
    }

    #[cfg(windows)]
    #[test]
    fn test_qir_to_qis_mz_leaked_windows_smoke() {
        let bc_bytes = load_fixture_bitcode("tests/data/mz_leaked.ll");
        let output_bc = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect("mz_leaked fixture should compile successfully on Windows");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "mz_leaked_qis")
            .expect("Compiled QIS bitcode should parse on Windows");
        assert!(module.get_function("qmain").is_some());
    }

    #[cfg(feature = "wasm")]
    proptest! {
        #[test]
        fn prop_get_wasm_functions_round_trips_exact_exports(
            exports in proptest::collection::btree_map("[A-Za-z_][A-Za-z0-9_]{0,8}", 0u32..32u32, 0..8)
        ) {
            let exports_vec = exports
                .iter()
                .map(|(name, index)| (name.clone(), *index))
                .collect::<Vec<_>>();
            let wasm = build_wasm_exports(&exports_vec);
            let parsed = crate::get_wasm_functions(Some(&wasm))
                .map_err(|err| TestCaseError::fail(format!("get_wasm_functions failed unexpectedly: {err}")))?;

            prop_assert_eq!(parsed.len(), exports.len());
            for (name, index) in exports {
                prop_assert_eq!(parsed.get(&name), Some(&u64::from(index)));
            }
        }
    }

    proptest! {
        #[cfg(not(windows))]
        #[test]
        fn prop_qir_ll_to_bc_rejects_malformed_ir(suffix in "\\PC{0,128}") {
            let ll_text = format!("this is not valid llvm ir\n{suffix}");
            prop_assert!(qir_ll_to_bc(&ll_text).is_err());
        }

        #[cfg(not(windows))]
        #[test]
        fn prop_validate_qir_rejects_malformed_bitcode(tail in proptest::collection::vec(any::<u8>(), 0..256)) {
            let mut bytes = b"NOTQIR".to_vec();
            bytes.extend(tail);
            prop_assert!(validate_qir(&bytes, None).is_err());
        }

        #[cfg(not(windows))]
        #[test]
        fn prop_qir_to_qis_rejects_malformed_bitcode(tail in proptest::collection::vec(any::<u8>(), 0..256)) {
            let mut bytes = b"NOTQIR".to_vec();
            bytes.extend(tail);
            let (opt_level, target) = conservative_translation_settings();
            prop_assert!(qir_to_qis(&bytes, opt_level, target, None).is_err());
        }

        #[test]
        fn prop_valid_fixtures_translate_to_verifiable_qis(fixture in proptest::sample::select(PROPERTY_FIXTURES)) {
            let input_bc = load_fixture_bitcode(fixture);
            prop_assert!(validate_qir(&input_bc, None).is_ok());

            let (opt_level, target) = conservative_translation_settings();
            let output_bc = qir_to_qis(&input_bc, opt_level, target, None)
                .map_err(|err| TestCaseError::fail(format!("translation failed for {fixture}: {err}")))?;

            verify_bitcode_module(&output_bc, "property_qis_module")
                .map_err(|err| TestCaseError::fail(format!("verification failed for {fixture}: {err}")))?;
        }

        #[test]
        fn prop_invalid_targets_fail_fast(target in "[a-z0-9_-]{1,12}") {
            prop_assume!(target != "native");
            prop_assume!(target != "aarch64");
            prop_assume!(target != "x86-64");

            let input_bc = load_fixture_bitcode("tests/data/base.ll");
            prop_assert!(qir_to_qis(&input_bc, 0, &target, None).is_err());
        }

        #[test]
        fn prop_missing_required_attrs_fail_validation(
            missing_idx in 0usize..4usize
        ) {
            let missing_attr = [
                "qir_profiles",
                "output_labeling_schema",
                "required_num_qubits",
                "required_num_results",
            ][missing_idx];
            let ll_text = minimal_qir_missing_attr(missing_attr);
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            let err = validate_qir(&bc, None)
                .expect_err("validation should reject missing required attributes");
            let expected = format!("Missing required attribute: `{missing_attr}`");
            prop_assert!(err.contains(&expected));
        }

        #[test]
        fn prop_qir_major_versions_accept_only_one_or_two(major in 0u32..5u32) {
            let ll_text = minimal_qir_with_body("1", "1", &major.to_string(), "", "");
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            let result = validate_qir(&bc, None);
            if matches!(major, 1 | 2) {
                prop_assert!(result.is_ok());
            } else {
                let err = result.expect_err("invalid major versions must fail");
                prop_assert!(err.contains("Unsupported qir_major_version"));
            }
        }

        #[test]
        fn prop_duplicate_qir_major_flags_pass_if_any_match(
            valid_first in any::<bool>(),
            valid_second in any::<bool>(),
        ) {
            let first_major = if valid_first { "1" } else { "99" };
            let second_major = if valid_second { "2" } else { "100" };
            let ll_text = minimal_qir_with_duplicate_major_flags(first_major, second_major);
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            let result = validate_qir(&bc, None);
            if valid_first || valid_second {
                prop_assert!(result.is_ok());
            } else {
                let err = result.expect_err("all-invalid duplicate major flags must fail");
                prop_assert!(err.contains("Unsupported qir_major_version"));
            }
        }

        #[test]
        fn prop_duplicate_dynamic_qubit_flags_accept_true_and_false_values(
            first_is_true in any::<bool>(),
            second_is_true in any::<bool>(),
        ) {
            let first_flag = if first_is_true { "true" } else { "false" };
            let second_flag = if second_is_true { "true" } else { "false" };
            let ll_text = minimal_qir_with_duplicate_dynamic_flags(first_flag, second_flag);
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            prop_assert!(validate_qir(&bc, None).is_ok());
        }

        #[test]
        fn prop_barrier_validation_tracks_required_qubits(
            required_num_qubits in 1u32..5u32,
            barrier_arity in 1u32..5u32,
        ) {
            let barrier_name = format!("__quantum__qis__barrier{barrier_arity}__body");
            let barrier_args = (0..barrier_arity)
                .map(|idx| format!("%Qubit* %q{idx}"))
                .collect::<Vec<_>>()
                .join(", ");
            let extra_decl = format!(
                "declare void @{barrier_name}({})",
                std::iter::repeat_n("%Qubit*", usize::try_from(barrier_arity).unwrap_or(0))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let body = (0..barrier_arity)
                .map(|idx| {
                    let one_based_idx = idx.saturating_add(1);
                    format!("  %q{idx} = inttoptr i64 {one_based_idx} to %Qubit*")
                })
                .chain(std::iter::once(format!("  call void @{barrier_name}({barrier_args})")))
                .collect::<Vec<_>>()
                .join("\n");
            let ll_text = minimal_qir_with_body(
                &required_num_qubits.to_string(),
                "1",
                "1",
                &extra_decl,
                &body,
            );
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            let result = validate_qir(&bc, None);
            if barrier_arity <= required_num_qubits {
                prop_assert!(result.is_ok());
            } else {
                let err = result.expect_err("oversized barrier arity must fail");
                prop_assert!(err.contains("Barrier arity"));
            }
        }

        #[cfg(not(windows))]
        #[test]
        fn prop_get_entry_attributes_rejects_malformed_bitcode(
            tail in proptest::collection::vec(any::<u8>(), 0..256)
        ) {
            let mut bytes = b"NOTQIR".to_vec();
            bytes.extend(tail);
            prop_assert!(get_entry_attributes(&bytes).is_err());
        }
    }

    #[test]
    fn test_zero_qubits_fail_validation() {
        let ll_text = minimal_qir_with_body("0", "1", "1", "", "");
        let bc = qir_ll_to_bc(&ll_text).expect("inline IR should parse");
        let err = validate_qir(&bc, None).expect_err("validation should reject zero qubits");
        assert!(err.contains("Entry function must have at least one qubit"));
    }

    #[test]
    fn test_validate_dynamic_qubits_without_required_num_qubits() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %err = alloca i1, align 1
  %q = call ptr @__quantum__rt__qubit_allocate(ptr %err)
  call void @__quantum__qis__h__body(ptr %q)
  call void @__quantum__rt__qubit_release(ptr %q)
  ret i64 0
}

declare ptr @__quantum__rt__qubit_allocate(ptr)
declare void @__quantum__rt__qubit_release(ptr)
declare void @__quantum__qis__h__body(ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 true}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        validate_qir(&bc_bytes, None).expect("dynamic qubit fixture should validate");
        let qis_bytes =
            qir_to_qis(&bc_bytes, 0, "native", None).expect("dynamic qubit fixture should compile");
        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &qis_bytes, "qis_module").unwrap();
        assert!(module.get_function("qir_qis.qubit_allocate").is_some());
        assert!(module.get_function("qir_qis.qubit_release").is_some());
    }

    #[test]
    fn test_validate_capability_usage_ignores_unused_rt_declarations() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_release(ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        validate_qir(&bc_bytes, None)
            .expect("unused dynamic result declarations should not fail validation");
    }

    #[test]
    fn test_validate_capability_usage_reports_called_rt_function_without_flag() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %err = alloca i1, align 1
  %r = call ptr @__quantum__rt__result_allocate(ptr %err)
  call void @__quantum__rt__result_release(ptr %r)
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_release(ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None)
            .expect_err("called dynamic result functions should fail validation");
        assert!(
            err.contains(
                "__quantum__rt__result_allocate requires `dynamic_result_management=true`"
            )
        );
    }

    #[test]
    fn test_validate_capability_usage_reports_called_dynamic_qubit_rt_function_without_flag() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %err = alloca i1, align 1
  %q = call ptr @__quantum__rt__qubit_allocate(ptr %err)
  call void @__quantum__rt__qubit_release(ptr %q)
  ret i64 0
}

declare ptr @__quantum__rt__qubit_allocate(ptr)
declare void @__quantum__rt__qubit_release(ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None)
            .expect_err("called dynamic qubit functions should fail validation");
        assert!(
            err.contains("__quantum__rt__qubit_allocate requires `dynamic_qubit_management=true`")
        );
    }

    #[test]
    fn test_validate_qir_rejects_malformed_dynamic_qubit_allocate_signature() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %q = call ptr @__quantum__rt__qubit_allocate()
  call void @__quantum__rt__qubit_release(ptr %q)
  ret i64 0
}

declare ptr @__quantum__rt__qubit_allocate()
declare void @__quantum__rt__qubit_release(ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 true}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None)
            .expect_err("malformed dynamic runtime declaration should fail validation");
        assert!(
            err.contains("Malformed QIR RT function declaration: __quantum__rt__qubit_allocate")
        );
    }

    #[test]
    fn test_validate_qir_rejects_unsupported_rt_function_declaration() {
        let ll_text = r#"
declare void @__quantum__rt__mystery()

define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#;

        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err =
            validate_qir(&bc_bytes, None).expect_err("unsupported RT declarations should fail");
        assert!(err.contains("Unsupported QIR RT function: __quantum__rt__mystery"));
    }

    #[test]
    fn test_validate_dynamic_results_without_required_num_results() {
        let ll_text = r#"
@0 = internal constant [3 x i8] c"r0\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__qis__mz__body(ptr null, ptr %r)
  call void @__quantum__rt__result_record_output(ptr %r, ptr @0)
  call void @__quantum__rt__result_release(ptr %r)
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_release(ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1
declare void @__quantum__rt__result_record_output(ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        validate_qir(&bc_bytes, None).expect("dynamic result fixture should validate");
        let qis_bytes = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect("dynamic result fixture should compile");
        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &qis_bytes, "qis_module").unwrap();
        assert!(module.get_function("qir_qis.result_read").is_some());
        assert!(module.get_function("qir_qis.out_err_success").is_some());
    }

    #[test]
    fn test_validate_dynamic_result_array_record_output() {
        let ll_text = r#"
@0 = internal constant [3 x i8] c"a0\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %results = alloca [2 x ptr], align 8
  call void @__quantum__rt__result_array_allocate(i64 2, ptr %results, ptr null)
  %r0_ptr = getelementptr inbounds [2 x ptr], ptr %results, i64 0, i64 0
  %r0 = load ptr, ptr %r0_ptr, align 8
  %r1_ptr = getelementptr inbounds [2 x ptr], ptr %results, i64 0, i64 1
  %r1 = load ptr, ptr %r1_ptr, align 8
  call void @__quantum__qis__mz__body(ptr null, ptr %r0)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr %r1)
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %results, ptr @0)
  call void @__quantum__rt__result_array_release(i64 2, ptr %results)
  ret i64 0
}

declare void @__quantum__rt__result_array_allocate(i64, ptr, ptr)
declare void @__quantum__rt__result_array_release(i64, ptr)
declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="2" }
attributes #1 = { "irreversible" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        validate_qir(&bc_bytes, None).expect("dynamic result array fixture should validate");
        let qis_bytes = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect("dynamic result array fixture should compile");
        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &qis_bytes, "qis_module").unwrap();
        assert!(
            module
                .get_function("qir_qis.result_array_record_output")
                .is_some()
        );
        assert!(module.get_function("qir_qis.out_err_success").is_some());

        #[cfg(not(windows))]
        {
            let module_text = module.to_string();
            assert!(
                module_text.contains("USER:RESULT_ARRAY:a0"),
                "expected RESULT_ARRAY output tag, got module:\n{module_text}"
            );
            let print_bool_arr_calls = module_text.matches("@print_bool_arr").count();
            assert!(
                print_bool_arr_calls >= 2,
                "expected print_bool_arr declaration and use, got module:\n{module_text}"
            );
            assert!(
                !module_text.contains("@print_bool("),
                "expected array output lowering without scalar print_bool fallback, got module:\n{module_text}"
            );
        }

        #[cfg(windows)]
        {
            let labels = module
                .get_globals()
                .filter_map(|global| crate::convert::get_string_label(global).ok())
                .collect::<Vec<_>>();
            assert!(
                labels
                    .iter()
                    .any(|label| label.contains("USER:RESULT_ARRAY:a0")),
                "expected RESULT_ARRAY output label in globals, got labels: {labels:?}"
            );
            assert!(module.get_function("print_bool_arr").is_some());
            assert!(module.get_function("print_bool").is_none());
        }
    }

    #[test]
    fn test_validate_dynamic_result_array_record_output_length_mismatch_fails() {
        let ll_text = r#"
@0 = internal constant [3 x i8] c"a0\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %results = alloca [1 x ptr], align 8
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %results, ptr @0)
  ret i64 0
}

declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None)
            .expect_err("mismatched result array output backing should fail validation");
        assert!(err.contains(
            "__quantum__rt__result_array_record_output requires a fixed-size backing array"
        ));
        assert!(err.contains("requested length 2 does not match backing array length 1"));
    }

    #[test]
    fn test_validate_dynamic_result_array_record_output_large_length_fails() {
        let ll_text = r#"
@0 = internal constant [3 x i8] c"a0\00"

define i64 @Entry_Point_Name() #0 {
entry:
  %results = alloca [2147483648 x ptr], align 8
  call void @__quantum__rt__result_array_record_output(i64 2147483648, ptr %results, ptr @0)
  ret i64 0
}

declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None)
            .expect_err("oversized result array output length should fail validation");
        assert!(err.contains(
            "__quantum__rt__result_array_record_output requires an array length that fits in i32 for RESULT_ARRAY output"
        ));
    }

    #[test]
    fn test_validate_dynamic_result_allocate_outside_entry_block_fails() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  br label %body

body:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__rt__result_release(ptr %r)
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_release(ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None).expect_err("fixture should fail validation");
        assert!(
            err.contains("__quantum__rt__result_allocate is only supported in the entry block")
        );
    }

    #[test]
    fn test_validate_dynamic_result_allocate_in_helper_fails() {
        let ll_text = r#"
define void @helper() {
entry:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__rt__result_release(ptr %r)
  ret void
}

define i64 @Entry_Point_Name() #0 {
entry:
  call void @helper()
  ret i64 0
}

declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_release(ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None).expect_err("fixture should fail validation");
        assert!(
            err.contains("__quantum__rt__result_allocate is only supported in the entry block")
        );
    }

    #[test]
    fn test_validate_input_defined_qir_qis_helper_fails() {
        let ll_text = r#"
define ptr @qir_qis.qubit_allocate(ptr %out_err) {
entry:
  ret ptr null
}

define i64 @Entry_Point_Name() #0 {
entry:
  ret i64 0
}

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 false}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None).expect_err("fixture should fail validation");
        assert!(err.contains("Input QIR must not define internal helper function"));
        assert!(err.contains("qir_qis.qubit_allocate"));
    }

    #[test]
    fn test_validate_dynamic_qubit_array_allocate_length_mismatch_fails() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %qubits = alloca [1 x ptr], align 8
  call void @__quantum__rt__qubit_array_allocate(i64 2, ptr %qubits, ptr null)
  ret i64 0
}

declare void @__quantum__rt__qubit_array_allocate(i64, ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 true}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None).expect_err("fixture should fail validation");
        assert!(
            err.contains("__quantum__rt__qubit_array_allocate requires a fixed-size backing array")
        );
        assert!(err.contains("requested length 2 does not match backing array length 1"));
    }

    #[test]
    fn test_validate_dynamic_result_array_allocate_length_mismatch_fails() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %results = alloca [1 x ptr], align 8
  call void @__quantum__rt__result_array_allocate(i64 2, ptr %results, ptr null)
  ret i64 0
}

declare void @__quantum__rt__result_array_allocate(i64, ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 false}
!3 = !{i32 1, !"dynamic_result_management", i1 true}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None).expect_err("fixture should fail validation");
        assert!(
            err.contains(
                "__quantum__rt__result_array_allocate requires a fixed-size backing array"
            )
        );
        assert!(err.contains("requested length 2 does not match backing array length 1"));
    }

    #[test]
    fn test_validate_dynamic_qubit_array_allocate_bitcast_backing_succeeds() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %qubits = alloca [2 x ptr], align 8
  %backing = bitcast ptr %qubits to ptr
  call void @__quantum__rt__qubit_array_allocate(i64 2, ptr %backing, ptr null)
  ret i64 0
}

declare void @__quantum__rt__qubit_array_allocate(i64, ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 true}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        validate_qir(&bc_bytes, None).expect("bitcast-backed qubit array should validate");
    }

    #[test]
    fn test_validate_dynamic_qubit_array_allocate_zero_gep_backing_succeeds() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %qubits = alloca [2 x ptr], align 8
  %backing = getelementptr inbounds [2 x ptr], ptr %qubits, i64 0, i64 0
  call void @__quantum__rt__qubit_array_allocate(i64 2, ptr %backing, ptr null)
  ret i64 0
}

declare void @__quantum__rt__qubit_array_allocate(i64, ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 true}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        validate_qir(&bc_bytes, None).expect("zero-index GEP backing should validate");
    }

    #[test]
    fn test_validate_dynamic_qubit_array_allocate_nonzero_gep_backing_fails() {
        let ll_text = r#"
define i64 @Entry_Point_Name() #0 {
entry:
  %qubits = alloca [2 x ptr], align 8
  %backing = getelementptr inbounds [2 x ptr], ptr %qubits, i64 0, i64 1
  call void @__quantum__rt__qubit_array_allocate(i64 2, ptr %backing, ptr null)
  ret i64 0
}

declare void @__quantum__rt__qubit_array_allocate(i64, ptr, ptr)

attributes #0 = { "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1" }

!llvm.module.flags = !{!0, !1, !2, !3, !4}
!0 = !{i32 1, !"qir_major_version", i32 2}
!1 = !{i32 7, !"qir_minor_version", i32 0}
!2 = !{i32 1, !"dynamic_qubit_management", i1 true}
!3 = !{i32 1, !"dynamic_result_management", i1 false}
!4 = !{i32 1, !"arrays", i1 true}
"#;
        let bc_bytes = qir_ll_to_bc(ll_text).unwrap();
        let err = validate_qir(&bc_bytes, None).expect_err(
            "non-zero GEP-backed pointer should not count as a fixed-size array backing",
        );
        assert!(err.contains(
            "__quantum__rt__qubit_array_allocate requires a fixed-size backing array allocated as [N x ptr]"
        ));
    }

    #[test]
    fn test_dynamic_qubit_array_allocate_rolls_back_on_failure() {
        let ll_text = std::fs::read_to_string("tests/data/dynamic_qubit_array_checked.ll")
            .expect("Failed to read dynamic_qubit_array_checked.ll");
        let input_bc =
            qir_ll_to_bc(&ll_text).expect("Failed to convert dynamic_qubit_array_checked.ll");
        let output_bc = qir_to_qis(&input_bc, 0, "native", None)
            .expect("dynamic qubit array checked fixture should compile");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "qis_module")
            .expect("Compiled QIS bitcode should parse");
        let helper = module
            .get_function("qir_qis.qubit_array_allocate")
            .expect("qubit array allocate helper should exist");
        let called_functions = collect_called_function_names(helper);
        assert!(
            called_functions
                .iter()
                .any(|name| name == "qir_qis.qubit_array_release"),
            "expected rollback release path in helper, got calls: {called_functions:?}"
        );
    }

    #[test]
    fn test_dynamic_qubit_array_allocate_initializes_out_err() {
        let ll_text = std::fs::read_to_string("tests/data/dynamic_qubit_array_checked.ll")
            .expect("Failed to read dynamic_qubit_array_checked.ll");
        let input_bc =
            qir_ll_to_bc(&ll_text).expect("Failed to convert dynamic_qubit_array_checked.ll");
        let output_bc = qir_to_qis(&input_bc, 0, "native", None)
            .expect("dynamic qubit array checked fixture should compile");

        let ctx = Context::create();
        let module = parse_bitcode_module(&ctx, &output_bc, "qis_module")
            .expect("Compiled QIS bitcode should parse");
        let helper = module
            .get_function("qir_qis.qubit_array_allocate")
            .expect("qubit array allocate helper should exist");
        let called_functions = collect_called_function_names(helper);
        assert!(
            called_functions
                .iter()
                .any(|name| name == "qir_qis.out_err_success"),
            "expected helper to initialize out_err, got calls: {called_functions:?}"
        );
    }

    #[test]
    fn test_dynamic_feature_fixtures_translate_to_verifiable_qis() {
        let (opt_level, target) = conservative_translation_settings();
        for fixture in DYNAMIC_FEATURE_FIXTURES {
            let ll_text = std::fs::read_to_string(fixture).expect("Failed to read fixture");
            let input_bc = qir_ll_to_bc(&ll_text).expect("Failed to convert fixture to bitcode");
            validate_qir(&input_bc, None).expect("Dynamic fixture should validate");
            let output_bc = qir_to_qis(&input_bc, opt_level, target, None)
                .expect("Dynamic fixture should translate");
            verify_bitcode_module(&output_bc, fixture)
                .expect("Dynamic fixture should remain LLVM-verifiable");
        }
    }

    proptest! {
        #[test]
        fn prop_dynamic_result_array_record_output_preserves_label(
            label in "[A-Za-z0-9_]{1,8}",
        ) {
            let label_len = label
                .len()
                .checked_add(1)
                .expect("label length bound should leave room for null terminator");
            let ll_text = format!(
                r#"
@0 = internal constant [{label_len} x i8] c"{label}\00"

define i64 @Entry_Point_Name() #0 {{
entry:
  %results = alloca [2 x ptr], align 8
  call void @__quantum__rt__result_array_allocate(i64 2, ptr %results, ptr null)
  %r0_ptr = getelementptr inbounds [2 x ptr], ptr %results, i64 0, i64 0
  %r0 = load ptr, ptr %r0_ptr, align 8
  %r1_ptr = getelementptr inbounds [2 x ptr], ptr %results, i64 0, i64 1
  %r1 = load ptr, ptr %r1_ptr, align 8
  call void @__quantum__qis__mz__body(ptr null, ptr %r0)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr %r1)
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %results, ptr @0)
  call void @__quantum__rt__result_array_release(i64 2, ptr %results)
  ret i64 0
}}

declare void @__quantum__rt__result_array_allocate(i64, ptr, ptr)
declare void @__quantum__rt__result_array_release(i64, ptr)
declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)
declare void @__quantum__qis__mz__body(ptr, ptr writeonly) #1

attributes #0 = {{ "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="2" }}
attributes #1 = {{ "irreversible" }}

!llvm.module.flags = !{{!0, !1, !2, !3, !4}}
!0 = !{{i32 1, !"qir_major_version", i32 2}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
!2 = !{{i32 1, !"dynamic_qubit_management", i1 false}}
!3 = !{{i32 1, !"dynamic_result_management", i1 true}}
!4 = !{{i32 1, !"arrays", i1 true}}
"#
            );

            let bc_bytes = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("Failed to lower inline IR: {err}")))?;
            validate_qir(&bc_bytes, None)
                .map_err(|err| TestCaseError::fail(format!("Fixture should validate: {err}")))?;
            let qis_bytes = qir_to_qis(&bc_bytes, 0, "native", None)
                .map_err(|err| TestCaseError::fail(format!("Fixture should compile: {err}")))?;
            let ctx = Context::create();
            let module = parse_bitcode_module(&ctx, &qis_bytes, "qis_module")
                .map_err(|err| TestCaseError::fail(format!("Compiled module should parse: {err}")))?;
            let expected_label = format!("USER:RESULT_ARRAY:{label}");
            let labels = module
                .get_globals()
                .filter_map(|global| get_string_label(global).ok())
                .collect::<Vec<_>>();

            prop_assert!(
                labels.iter().any(|item| item.contains(&expected_label)),
                "expected label payload {expected_label}, got globals: {labels:?}"
            );
        }

        #[test]
        fn prop_dynamic_array_allocate_requires_matching_fixed_backing(
            backing_len in 1u32..4u32,
            requested_len in 1u32..4u32,
            is_result_array in any::<bool>(),
        ) {
            let (rt_decl, call, attrs, flags) = if is_result_array {
                (
                    "declare void @__quantum__rt__result_array_allocate(i64, ptr, ptr)",
                    format!(
                        "  %results = alloca [{backing_len} x ptr], align 8\n  call void @__quantum__rt__result_array_allocate(i64 {requested_len}, ptr %results, ptr null)"
                    ),
                    r#""entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1""#,
                    "!2 = !{i32 1, !\"dynamic_qubit_management\", i1 false}\n!3 = !{i32 1, !\"dynamic_result_management\", i1 true}\n!4 = !{i32 1, !\"arrays\", i1 true}",
                )
            } else {
                (
                    "declare void @__quantum__rt__qubit_array_allocate(i64, ptr, ptr)",
                    format!(
                        "  %qubits = alloca [{backing_len} x ptr], align 8\n  call void @__quantum__rt__qubit_array_allocate(i64 {requested_len}, ptr %qubits, ptr null)"
                    ),
                    r#""entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1""#,
                    "!2 = !{i32 1, !\"dynamic_qubit_management\", i1 true}\n!3 = !{i32 1, !\"dynamic_result_management\", i1 false}\n!4 = !{i32 1, !\"arrays\", i1 true}",
                )
            };

            let ll_text = format!(
                r#"
define i64 @Entry_Point_Name() #0 {{
entry:
{call}
  ret i64 0
}}

{rt_decl}

attributes #0 = {{ {attrs} }}

!llvm.module.flags = !{{!0, !1, !2, !3, !4}}
!0 = !{{i32 1, !"qir_major_version", i32 2}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
{flags}
"#
            );
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            let result = validate_qir(&bc, None);
            if backing_len == requested_len {
                prop_assert!(result.is_ok());
            } else {
                let err = result.expect_err("mismatched fixed-size backing should fail");
                prop_assert!(err.contains("requires a fixed-size backing array"));
                prop_assert!(err.contains("requested length"));
            }
        }

        #[test]
        fn prop_dynamic_array_runtime_calls_require_both_arrays_and_matching_dynamic_flag(
            arrays_enabled in any::<bool>(),
            dynamic_enabled in any::<bool>(),
            is_result_array in any::<bool>(),
        ) {
            let (call, rt_decl, attrs, flags, expected_error) = if is_result_array {
                (
                    "  %results = alloca [2 x ptr], align 8\n  call void @__quantum__rt__result_array_allocate(i64 2, ptr %results, ptr null)",
                    "declare void @__quantum__rt__result_array_allocate(i64, ptr, ptr)",
                    r#""entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="1""#.to_string(),
                    format!(
                        "!2 = !{{i32 1, !\"dynamic_qubit_management\", i1 false}}\n!3 = !{{i32 1, !\"dynamic_result_management\", i1 {dynamic_enabled}}}\n!4 = !{{i32 1, !\"arrays\", i1 {arrays_enabled}}}"
                    ),
                    "__quantum__rt__result_array_allocate requires both `arrays=true` and `dynamic_result_management=true`",
                )
            } else {
                (
                    "  %qubits = alloca [2 x ptr], align 8\n  call void @__quantum__rt__qubit_array_allocate(i64 2, ptr %qubits, ptr null)",
                    "declare void @__quantum__rt__qubit_array_allocate(i64, ptr, ptr)",
                    r#""entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_results"="1""#.to_string(),
                    format!(
                        "!2 = !{{i32 1, !\"dynamic_qubit_management\", i1 {dynamic_enabled}}}\n!3 = !{{i32 1, !\"dynamic_result_management\", i1 false}}\n!4 = !{{i32 1, !\"arrays\", i1 {arrays_enabled}}}"
                    ),
                    "__quantum__rt__qubit_array_allocate requires both `arrays=true` and `dynamic_qubit_management=true`",
                )
            };

            let ll_text = format!(
                r#"
define i64 @Entry_Point_Name() #0 {{
entry:
{call}
  ret i64 0
}}

{rt_decl}

attributes #0 = {{ {attrs} }}

!llvm.module.flags = !{{!0, !1, !2, !3, !4}}
!0 = !{{i32 1, !"qir_major_version", i32 2}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
{flags}
"#
            );
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            let result = validate_qir(&bc, None);
            if arrays_enabled && dynamic_enabled {
                prop_assert!(result.is_ok());
            } else {
                let err = result.expect_err("missing capability flag combination should fail");
                prop_assert!(err.contains(expected_error));
            }
        }
    }
}
