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
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use crate::{
        convert::{
            INIT_QARRAY_FN, LOAD_QUBIT_FN, add_print_call, build_result_global, convert_globals,
            create_reset_call, get_index, get_or_create_function, get_required_num_qubits,
            get_required_num_qubits_strict, get_result_vars, get_string_label,
            handle_tuple_or_array_output, parse_gep, record_classical_output, replace_rxy_call,
            replace_rz_call, replace_rzz_call,
        },
        decode_llvm_bytes,
        utils::extract_operands,
    };

    use inkwell::{
        AddressSpace,
        attributes::AttributeLoc,
        context::Context,
        module::Module,
        types::{ArrayType, BasicTypeEnum},
        values::{
            AnyValue, BasicMetadataValueEnum, BasicValue, BasicValueEnum, CallSiteValue,
            FunctionValue, PointerValue,
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

    static ALLOWED_RT_FNS: [&str; 8] = [
        "__quantum__rt__read_result",
        "__quantum__rt__initialize",
        "__quantum__rt__result_record_output",
        "__quantum__rt__array_record_output",
        "__quantum__rt__tuple_record_output",
        "__quantum__rt__bool_record_output",
        "__quantum__rt__double_record_output",
        "__quantum__rt__int_record_output",
    ];

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
                if !ALLOWED_RT_FNS.contains(&fn_name) {
                    errors.push(format!("Unsupported QIR RT function: {fn_name}"));
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
            &["i1 false"],
            errors,
        );
        validate_exact_module_flag(
            &module_flags,
            "dynamic_result_management",
            &["i1 false"],
            errors,
        );
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

    #[allow(dead_code)]
    struct ProcessCallArgs<'a, 'ctx> {
        ctx: &'ctx Context,
        module: &'a Module<'ctx>,
        instr: &'a inkwell::values::InstructionValue<'ctx>,
        fn_name: &'a str,
        wasm_fns: &'a BTreeMap<String, u64>,
        qubit_array: PointerValue<'ctx>,
        qubit_array_type: ArrayType<'ctx>,
        global_mapping: &'a mut HashMap<String, inkwell::values::GlobalValue<'ctx>>,
        result_ssa: &'a mut [Option<(BasicValueEnum<'ctx>, Option<BasicValueEnum<'ctx>>)>],
    }

    /// Primary translation loop over the entry function for translation to QIS.
    pub fn process_entry_function<'ctx>(
        ctx: &'ctx Context,
        module: &Module<'ctx>,
        entry_fn: FunctionValue<'ctx>,
        wasm_fns: &BTreeMap<String, u64>,
        qubit_array: PointerValue<'ctx>,
    ) -> Result<(), String> {
        let mut global_mapping = convert_globals(ctx, module)?;

        if global_mapping.is_empty() {
            log::warn!("No globals found in QIR module");
        }
        let mut result_ssa = get_result_vars(entry_fn)?;
        let required_num_qubits = get_required_num_qubits_strict(entry_fn)?;
        let qubit_array_type = ctx.i64_type().array_type(required_num_qubits);

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
                    let mut args = ProcessCallArgs {
                        ctx,
                        module,
                        instr: &instr,
                        fn_name: &fn_name,
                        wasm_fns,
                        qubit_array,
                        qubit_array_type,
                        global_mapping: &mut global_mapping,
                        result_ssa: &mut result_ssa,
                    };
                    process_call_instruction(&mut args)?;
                }
            }
        }

        Ok(())
    }

    fn process_call_instruction(args: &mut ProcessCallArgs) -> Result<(), String> {
        let call = CallSiteValue::try_from(*args.instr)
            .map_err(|()| "Instruction is not a call site".to_string())?;
        match args.fn_name {
            name if name.starts_with("__quantum__qis__") => handle_qis_call(args),
            name if name.starts_with("__quantum__rt__") => handle_rt_call(args),
            name if name.starts_with("___") => handle_qtm_call(args),
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

    fn handle_qis_call(args: &mut ProcessCallArgs) -> Result<(), String> {
        match args.fn_name {
            "__quantum__qis__rxy__body" => {
                replace_rxy_call(args.ctx, args.module, *args.instr)?;
            }
            "__quantum__qis__rz__body" => {
                replace_rz_call(args.ctx, args.module, *args.instr)?;
            }
            "__quantum__qis__rzz__body" => {
                replace_rzz_call(args.ctx, args.module, *args.instr)?;
            }
            "__quantum__qis__u1q__body" => {
                log::info!(
                    "`__quantum__qis__u1q__body` used, synonym for `__quantum__qis__rxy__body`"
                );
                replace_rxy_call(args.ctx, args.module, *args.instr)?;
            }
            "__quantum__qis__mz__body"
            | "__quantum__qis__m__body"
            | "__quantum__qis__mresetz__body" => {
                handle_mz_call(args)?;
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
                let is_ir_defined = args
                    .module
                    .get_function(args.fn_name)
                    .is_some_and(|f| f.count_basic_blocks() > 0);
                if !is_ir_defined {
                    return Err(format!("Unsupported QIR QIS function: {}", args.fn_name));
                }
            }
        }
        Ok(())
    }

    fn handle_rt_call(args: &mut ProcessCallArgs) -> Result<(), String> {
        match args.fn_name {
            "__quantum__rt__initialize" => {
                args.instr.erase_from_basic_block();
            }
            "__quantum__rt__read_result" | "__quantum__rt__result_record_output" => {
                handle_read_result_call(args)?;
            }
            "__quantum__rt__tuple_record_output" | "__quantum__rt__array_record_output" => {
                handle_tuple_or_array_output(
                    args.ctx,
                    args.module,
                    *args.instr,
                    args.global_mapping,
                    args.fn_name,
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

    fn handle_qtm_call(args: &mut ProcessCallArgs) -> Result<(), String> {
        match args.fn_name {
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

    fn handle_mz_call(args: &mut ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx,
            module,
            instr,
            fn_name,
            qubit_array,
            qubit_array_type,
            result_ssa,
            ..
        } = args;

        if *fn_name == "__quantum__qis__m__body" {
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

        // Load qubit handle
        let q_handle = {
            let i64_type = ctx.i64_type();
            let index = get_index(qubit_ptr)?;
            let index_val = i64_type.const_int(index, false);
            let elem_ptr = unsafe {
                builder.build_gep(
                    *qubit_array_type,
                    *qubit_array,
                    &[i64_type.const_zero(), index_val],
                    "",
                )
            }
            .map_err(|e| format!("Failed to build GEP for qubit handle: {e}"))?;
            builder
                .build_load(i64_type, elem_ptr, "qbit")
                .map_err(|e| format!("Failed to build load for qubit handle: {e}"))?
        };

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
        let result_idx = get_index(result_ptr)?;
        let result_idx_usize = checked_result_index(result_idx, result_ssa.len())?;
        result_ssa[result_idx_usize] = Some((meas, None));

        if *fn_name == "__quantum__qis__mresetz__body" {
            log::warn!("`__quantum__qis__mresetz__body` is from Q# QDK");
            // Create ___reset call
            create_reset_call(ctx, module, &builder, q_handle);
        }

        // Remove original call
        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_mz_leaked_call(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx,
            module,
            instr,
            qubit_array,
            qubit_array_type,
            ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let qubit_ptr = call_args[0].into_pointer_value();

        let q_handle = {
            let i64_type = ctx.i64_type();
            let index = get_index(qubit_ptr)?;
            let index_val = i64_type.const_int(index, false);
            let elem_ptr = unsafe {
                builder.build_gep(
                    *qubit_array_type,
                    *qubit_array,
                    &[i64_type.const_zero(), index_val],
                    "",
                )
            }
            .map_err(|e| format!("Failed to build GEP for qubit handle: {e}"))?;
            builder
                .build_load(i64_type, elem_ptr, "qbit")
                .map_err(|e| format!("Failed to build load for qubit handle: {e}"))?
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

    fn handle_reset_call(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx, module, instr, ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        // Extract qubit index
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let qubit_ptr = call_args[0].into_pointer_value();

        // Load qubit handle
        let idx_fn = module
            .get_function(LOAD_QUBIT_FN)
            .ok_or_else(|| format!("{LOAD_QUBIT_FN} not found"))?;
        let idx_call = builder
            .build_call(idx_fn, &[qubit_ptr.into()], "qbit")
            .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}"))?;
        let q_handle = match idx_call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv,
            inkwell::values::ValueKind::Instruction(_) => {
                return Err(format!(
                    "Failed to get basic value from {LOAD_QUBIT_FN} call"
                ));
            }
        };

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
    fn handle_barrier_call(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx,
            module,
            instr,
            fn_name,
            ..
        } = args;
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

        let idx_fn = module
            .get_function(LOAD_QUBIT_FN)
            .ok_or_else(|| format!("{LOAD_QUBIT_FN} not found"))?;

        for (i, arg) in call_args.iter().enumerate() {
            let qubit_ptr = arg.into_pointer_value();
            let idx_call = builder
                .build_call(idx_fn, &[qubit_ptr.into()], "qbit")
                .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}"))?;
            let q_handle = match idx_call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err(format!(
                        "Failed to get basic value from {LOAD_QUBIT_FN} call"
                    ));
                }
            };

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

    fn handle_read_result_call(args: &mut ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx,
            module,
            instr,
            fn_name,
            global_mapping,
            result_ssa,
            ..
        } = args;
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let result_ptr = call_args[0].into_pointer_value();
        let result_idx = get_index(result_ptr)?;
        let result_idx_usize = checked_result_index(result_idx, result_ssa.len())?;
        let meas_handle = result_ssa[result_idx_usize]
            .ok_or_else(|| "Expected measurement handle".to_string())?;

        let builder = ctx.create_builder();
        builder.position_before(instr);

        // Compute or reuse the bool value for this meas_handle
        let bool_val = result_ssa[result_idx_usize]
            .and_then(|v| v.1)
            .and_then(|val| val.as_instruction_value())
            .map_or_else(
                || {
                    let read_func = get_or_create_function(
                        module,
                        "___read_future_bool",
                        ctx.bool_type().fn_type(&[ctx.i64_type().into()], false),
                    );
                    let bool_call = builder
                        .build_call(read_func, &[meas_handle.0.into()], "bool")
                        .map_err(|e| format!("Failed to build call for read_future_bool: {e}"))?;
                    let bool_val = match bool_call.try_as_basic_value() {
                        inkwell::values::ValueKind::Basic(bv) => bv,
                        inkwell::values::ValueKind::Instruction(_) => {
                            return Err(
                                "Failed to get basic value from read_future_bool call".into()
                            );
                        }
                    };

                    // Decrement refcount
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

                    // Store the result in SSA for reuse
                    result_ssa[result_idx_usize] = Some((meas_handle.0, Some(bool_val)));
                    Ok(bool_val)
                },
                |val| {
                    val.as_any_value_enum()
                        .try_into()
                        .map_err(|()| "Expected BasicValueEnum".to_string())
                },
            )?;

        if *fn_name == "__quantum__rt__read_result" {
            let instruction_val = bool_val
                .as_instruction_value()
                .ok_or("Failed to convert bool_val to instruction value")?;
            instr.replace_all_uses_with(&instruction_val);
        } else {
            // "__quantum__rt__result_record_output"
            let gep = call_args[1];
            let new_global = if let Ok(old_global) = parse_gep(gep) {
                global_mapping
                    .get(old_global.as_str())
                    .copied()
                    .ok_or_else(|| format!("Output global `{old_global}` not found in mapping"))?
            } else {
                let fallback_label = format!("result_{result_idx}");
                let (new_const, new_name) =
                    build_result_global(ctx, &fallback_label, &fallback_label, "RESULT", None)?;
                let new_global = module.add_global(new_const.get_type(), None, &new_name);
                new_global.set_initializer(&new_const);
                new_global.set_linkage(inkwell::module::Linkage::Private);
                new_global.set_constant(true);
                global_mapping.insert(fallback_label, new_global);
                new_global
            };

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

    fn handle_classical_record_output(args: &mut ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx,
            module,
            instr,
            fn_name,
            global_mapping,
            ..
        } = args;
        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let (print_func_name, value, type_tag) = match *fn_name {
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
        record_classical_output(ctx, **instr, new_global, print_func, value)?;
        Ok(())
    }

    fn handle_get_current_shot(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx, module, instr, ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let get_shot_func = get_or_create_function(
            module,
            // fun get_current_shot() -> uint
            "get_current_shot",
            ctx.i64_type().fn_type(&[], false),
        );

        let shot_call = builder
            .build_call(get_shot_func, &[], "current_shot")
            .map_err(|e| format!("Failed to build call to get_current_shot: {e}"))?;

        let shot_result = match shot_call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv,
            inkwell::values::ValueKind::Instruction(_) => {
                return Err("Failed to get basic value from get_current_shot call".into());
            }
        };

        if let Some(instr_val) = shot_result.as_instruction_value() {
            instr.replace_all_uses_with(&instr_val);
        }

        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_random_seed(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx, module, instr, ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let seed = call_args[0];

        let random_seed_func = get_or_create_function(
            module,
            // void random_seed(uint64_t seq)
            "random_seed",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );

        let _ = builder
            .build_call(random_seed_func, &[seed.into()], "")
            .map_err(|e| format!("Failed to build call to random_seed: {e}"))?;

        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_random_int(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx, module, instr, ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let random_int_func = get_or_create_function(
            module,
            // uint32_t random_int()
            "random_int",
            ctx.i32_type().fn_type(&[], false),
        );

        let random_call = builder
            .build_call(random_int_func, &[], "rint")
            .map_err(|e| format!("Failed to build call to random_int: {e}"))?;

        let random_result = match random_call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv,
            inkwell::values::ValueKind::Instruction(_) => {
                return Err("Failed to get basic value from random_int call".into());
            }
        };

        if let Some(instr_val) = random_result.as_instruction_value() {
            instr.replace_all_uses_with(&instr_val);
        }

        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_random_float(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx, module, instr, ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let random_float_func = get_or_create_function(
            module,
            // double random_float()
            "random_float",
            ctx.f64_type().fn_type(&[], false),
        );

        let random_call = builder
            .build_call(random_float_func, &[], "rfloat")
            .map_err(|e| format!("Failed to build call to random_float: {e}"))?;

        let random_result = match random_call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv,
            inkwell::values::ValueKind::Instruction(_) => {
                return Err("Failed to get basic value from random_float call".into());
            }
        };

        if let Some(instr_val) = random_result.as_instruction_value() {
            instr.replace_all_uses_with(&instr_val);
        }

        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_random_int_bounded(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx, module, instr, ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let bound = call_args[0];

        let random_rng_func = get_or_create_function(
            module,
            // uint32_t random_rng(uint32_t bound)
            "random_rng",
            ctx.i32_type().fn_type(&[ctx.i32_type().into()], false),
        );

        let rng_call = builder
            .build_call(random_rng_func, &[bound.into()], "rintb")
            .map_err(|e| format!("Failed to build call to random_rng: {e}"))?;

        let rng_result = match rng_call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => bv,
            inkwell::values::ValueKind::Instruction(_) => {
                return Err("Failed to get basic value from random_rng call".into());
            }
        };

        if let Some(instr_val) = rng_result.as_instruction_value() {
            instr.replace_all_uses_with(&instr_val);
        }

        instr.erase_from_basic_block();
        Ok(())
    }

    fn handle_random_advance(args: &ProcessCallArgs) -> Result<(), String> {
        let ProcessCallArgs {
            ctx, module, instr, ..
        } = args;
        let builder = ctx.create_builder();
        builder.position_before(instr);

        let call_args: Vec<BasicValueEnum> = extract_operands(instr)?;
        let delta = call_args[0];

        let random_advance_func = get_or_create_function(
            module,
            // void random_advance(uint64_t delta)
            "random_advance",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        );

        let _ = builder
            .build_call(random_advance_func, &[delta.into()], "")
            .map_err(|e| format!("Failed to build call to random_advance: {e}"))?;

        instr.erase_from_basic_block();
        Ok(())
    }
}

pub(crate) fn decode_llvm_bytes(value: &[u8]) -> Option<&str> {
    std::str::from_utf8(value).ok()
}

pub(crate) fn decode_llvm_c_string(value: &std::ffi::CStr) -> Option<&str> {
    value.to_str().ok()
}

pub(crate) fn create_module_from_ir_text<'ctx>(
    ctx: &'ctx inkwell::context::Context,
    ll_text: &str,
    name: &str,
) -> Result<inkwell::module::Module<'ctx>, String> {
    let ll_bytes = ll_text.as_bytes();
    let memory_buffer = if ll_bytes.ends_with(&[0]) {
        inkwell::memory_buffer::MemoryBuffer::create_from_memory_range_copy(ll_bytes, name)
    } else {
        let mut bytes = Vec::with_capacity(ll_bytes.len().saturating_add(1));
        bytes.extend_from_slice(ll_bytes);
        bytes.push(0);
        inkwell::memory_buffer::MemoryBuffer::create_from_memory_range_copy(&bytes, name)
    };
    ctx.create_module_from_ir(memory_buffer)
        .map_err(|e| format!("Failed to create module from LLVM IR: {e}"))
}

pub(crate) fn parse_bitcode_module<'ctx>(
    ctx: &'ctx inkwell::context::Context,
    bitcode: &[u8],
    name: &str,
) -> Result<inkwell::module::Module<'ctx>, String> {
    let memory_buffer = if bitcode.ends_with(&[0]) {
        inkwell::memory_buffer::MemoryBuffer::create_from_memory_range_copy(bitcode, name)
    } else {
        let mut bytes = bitcode.to_vec();
        bytes.push(0);
        inkwell::memory_buffer::MemoryBuffer::create_from_memory_range_copy(&bytes, name)
    };
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
        aux::process_entry_function,
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

    log::trace!("Entry function: {entry_fn_name}");
    let new_name = format!("___user_qir_{entry_fn_name}");
    entry_fn.as_global_value().set_name(&new_name);
    log::debug!("Renamed entry function to: {new_name}");
    let qubit_array = create_qubit_array(&ctx, &module, entry_fn)?;

    let wasm_fns: BTreeMap<String, u64> = BTreeMap::new();
    process_entry_function(&ctx, &module, entry_fn, &wasm_fns, qubit_array)?;

    // Handle IR defined functions that take qubits
    process_ir_defined_q_fns(&ctx, &module, entry_fn)?;

    free_all_qubits(&ctx, &module, entry_fn, qubit_array)?;

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

    Ok(module.write_bitcode_to_memory().as_slice().to_vec())
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
        aux::{validate_functions, validate_module_flags, validate_module_layout_and_triple},
        convert::{ENTRY_ATTRIBUTE_KEYS, find_entry_function},
    };
    use inkwell::{attributes::AttributeLoc, context::Context};

    let ctx = Context::create();
    let module = parse_bitcode_module(&ctx, bc_bytes, "bitcode")?;
    let mut errors = Vec::new();

    validate_module_layout_and_triple(&module);

    let entry_fn = if let Ok(entry_fn) = find_entry_function(&module) {
        if entry_fn.get_basic_blocks().is_empty() {
            errors.push("Entry function has no basic blocks".to_string());
        }

        // Enforce required attributes
        for attr in ENTRY_ATTRIBUTE_KEYS
            .iter()
            .copied()
            .filter(|attr| *attr != "entry_point")
        {
            let val = entry_fn.get_string_attribute(AttributeLoc::Function, attr);
            if val.is_none() {
                errors.push(format!("Missing required attribute: `{attr}`"));
            }
        }

        // `required_num_qubits` must stay positive. `required_num_results`
        // may be zero for programs that only use classical-returning operations
        // such as `mz_leaked`.
        for (attr, type_) in [("required_num_qubits", "qubit")] {
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

    validate_module_flags(&module, &mut errors);

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

    Ok(module.write_bitcode_to_memory().as_slice().to_vec())
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
        create_module_from_ir_text, get_entry_attributes, parse_bitcode_module, qir_ll_to_bc,
        qir_to_qis, validate_qir,
    };
    use inkwell::context::Context;
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
    fn test_qir_to_qis_rejects_zero_required_num_results_for_result_measurement() {
        let ll_text = minimal_qir_with_body(
            "1",
            "0",
            "1",
            "declare void @__quantum__qis__mz__body(%Qubit*, %Result* writeonly)",
            r"  call void @__quantum__qis__mz__body(%Qubit* null, %Result* writeonly null)",
        );

        let bc_bytes = qir_ll_to_bc(&ll_text).expect("Failed to convert inline QIR to bitcode");
        let err = qir_to_qis(&bc_bytes, 0, "native", None)
            .expect_err("result-backed measurements should still require declared result slots");
        assert!(err.contains("Result index 0 exceeds required_num_results (0)"));
    }

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
        fn prop_duplicate_dynamic_qubit_flags_pass_if_any_match(
            valid_first in any::<bool>(),
            valid_second in any::<bool>(),
        ) {
            let first_flag = if valid_first { "false" } else { "true" };
            let second_flag = if valid_second { "false" } else { "true" };
            let ll_text = minimal_qir_with_duplicate_dynamic_flags(first_flag, second_flag);
            let bc = qir_ll_to_bc(&ll_text)
                .map_err(|err| TestCaseError::fail(format!("inline IR should parse: {err}")))?;
            let result = validate_qir(&bc, None);
            if valid_first || valid_second {
                prop_assert!(result.is_ok());
            } else {
                let err = result.expect_err("all-invalid duplicate dynamic flags must fail");
                prop_assert!(err.contains("dynamic_qubit_management"));
            }
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
}
