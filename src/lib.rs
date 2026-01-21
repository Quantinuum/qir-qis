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
pub mod decompose;
pub mod opt;
pub mod utils;

mod aux {
    use std::collections::{BTreeMap, HashMap};

    use crate::{
        convert::{
            INIT_QARRAY_FN, LOAD_QUBIT_FN, add_print_call, build_result_global, convert_globals,
            create_reset_call, get_index, get_or_create_function, get_result_vars,
            get_string_label, handle_tuple_or_array_output, parse_gep, record_classical_output,
            replace_rxy_call, replace_rz_call, replace_rzz_call,
        },
        utils::extract_operands,
    };

    use inkwell::{
        AddressSpace,
        attributes::AttributeLoc,
        context::Context,
        module::Module,
        types::BasicTypeEnum,
        values::{
            AnyValue, BasicMetadataValueEnum, BasicValue, BasicValueEnum, CallSiteValue,
            FunctionValue, PointerValue,
        },
    };

    static ALLOWED_QIS_FNS: [&str; 22] = [
        // Native gates
        "__quantum__qis__rxy__body",
        "__quantum__qis__rz__body",
        "__quantum__qis__rzz__body",
        "__quantum__qis__mz__body",
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
    static ALLOWED_QTM_FNS: [&str; 7] = [
        "___get_current_shot",
        "___random_seed",
        "___random_int",
        "___random_float",
        "___random_int_bounded",
        "___random_advance",
        "___get_wasm_context",
    ];

    #[cfg(not(feature = "wasm"))]
    static ALLOWED_QTM_FNS: [&str; 6] = [
        "___get_current_shot",
        "___random_seed",
        "___random_int",
        "___random_float",
        "___random_int_bounded",
        "___random_advance",
    ];

    const REQ_FLAG_COUNT: usize = 4;

    /// The module flags that we care about are:
    /// 0: `qir_major_version`
    /// 1: `qir_minor_version`
    /// 2: `dynamic_qubit_management`
    /// 3: `dynamic_result_management`
    #[derive(Default)]
    struct ModuleFlagState {
        found: [bool; REQ_FLAG_COUNT],
        wrong: [bool; REQ_FLAG_COUNT],
        required: [(usize, &'static str, u64, &'static str); REQ_FLAG_COUNT],
    }

    impl ModuleFlagState {
        /// Create a new `ModuleFlagState` with the required flags.
        const fn new() -> Self {
            Self {
                found: [false; REQ_FLAG_COUNT],
                wrong: [false; REQ_FLAG_COUNT],
                required: [
                    // (index, name, expected_value, expected_str)
                    (0, "qir_major_version", 1, "1"),
                    (1, "qir_minor_version", 0, "0"),
                    (2, "dynamic_qubit_management", 0, "false"),
                    (3, "dynamic_result_management", 0, "false"),
                ],
            }
        }

        /// Set the flag state for a given module flag.
        fn set_state(&mut self, index: usize, value: &BasicMetadataValueEnum, expected: u64) {
            self.found[index] = true;
            self.wrong[index] = !value.is_int_value()
                || value.into_int_value().get_zero_extended_constant() != Some(expected);
        }
    }

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

    pub fn validate_functions(
        module: &Module,
        entry_fn: FunctionValue,
        _wasm_fns: &BTreeMap<String, u64>,
        errors: &mut Vec<String>,
    ) {
        for fun in module.get_functions() {
            if fun == entry_fn {
                // Skip the entry function
                continue;
            }
            let fn_name = fun.get_name().to_str().unwrap_or("");
            if fn_name.starts_with("__quantum__qis__") {
                if !ALLOWED_QIS_FNS.contains(&fn_name) {
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
        let mut mflags = ModuleFlagState::new();

        for md in module.get_global_metadata("llvm.module.flags") {
            if let [_, key, value] = md.get_node_values().as_slice()
                && let Some(key_str) = key
                    .into_metadata_value()
                    .get_string_value()
                    .and_then(|s| s.to_str().ok())
                && let Some((idx, _, expected, _)) = mflags
                    .required
                    .iter()
                    .find(|(_, name, _, _)| *name == key_str)
            {
                mflags.set_state(*idx, value, *expected);
            }
        }

        for (idx, name, _, expected_str) in mflags.required {
            if !mflags.found[idx] {
                errors.push(format!("Missing required module flag: `{name}`"));
            } else if mflags.wrong[idx] {
                errors.push(format!("Module flag `{name}` must be {expected_str}"));
            }
        }
    }

    #[allow(dead_code)]
    struct ProcessCallArgs<'a, 'ctx> {
        ctx: &'ctx Context,
        module: &'a Module<'ctx>,
        instr: &'a inkwell::values::InstructionValue<'ctx>,
        fn_name: &'a str,
        wasm_fns: &'a BTreeMap<String, u64>,
        qubit_array: PointerValue<'ctx>,
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

        for bb in entry_fn.get_basic_blocks() {
            for instr in bb.get_instructions() {
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
            "__quantum__qis__reset__body" => {
                handle_reset_call(args)?;
            }
            _ => return Err(format!("Unsupported QIR QIS function: {}", args.fn_name)),
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
            let elem_ptr =
                unsafe { builder.build_gep(*qubit_array, &[i64_type.const_zero(), index_val], "") }
                    .map_err(|e| format!("Failed to build GEP for qubit handle: {e}",))?;
            builder
                .build_load(elem_ptr, "qbit")
                .map_err(|e| format!("Failed to build load for qubit handle: {e}",))?
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
                call.map_err(|e| format!("Failed to build call for lazy measure function: {e}",))?;
            match call_result.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err("Failed to get basic value from lazy measure call".into());
                }
            }
        };

        // Store measurement result
        let result_idx = get_index(result_ptr)?;
        let result_idx_usize = usize::try_from(result_idx)
            .map_err(|e| format!("Failed to convert result index to usize: {e}"))?;
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
            .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}",))?;
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
        let meas_handle = result_ssa[usize::try_from(result_idx)
            .map_err(|e| format!("Failed to convert result index to usize: {e}"))?]
        .ok_or_else(|| "Expected measurement handle".to_string())?;

        let builder = ctx.create_builder();
        builder.position_before(instr);

        // Compute or reuse the bool value for this meas_handle
        let result_idx_usize = usize::try_from(result_idx)
            .map_err(|e| format!("Failed to convert result index to usize: {e}"))?;
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
            let old_global = parse_gep(gep)?;
            let new_global = global_mapping[old_global.as_str()];

            let print_func = get_or_create_function(
                module,
                "print_bool",
                ctx.void_type().fn_type(
                    &[
                        ctx.i8_type().ptr_type(AddressSpace::default()).into(), // i8*
                        ctx.i64_type().into(),                                  // i64
                        ctx.bool_type().into(),                                 // i1
                    ],
                    false,
                ),
            );

            add_print_call(ctx, &builder, new_global, print_func, bool_val)?;
        }
        instr.erase_from_basic_block();
        Ok(())
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
            ctx.i8_type().ptr_type(AddressSpace::default()).into(), // i8*
            ctx.i64_type().into(),                                  // i64
            match type_tag {
                "BOOL" => ctx.bool_type().into(),
                "INT" => ctx.i64_type().into(),
                "FLOAT" => ctx.f64_type().into(),
                _ => unreachable!(),
            },
        ];
        let fn_type = ret_type.fn_type(param_types, false);

        let print_func = get_or_create_function(module, print_func_name, fn_type);

        let old_global = parse_gep(call_args[1])?;
        let old_name = old_global.as_str();

        let full_tag = get_string_label(global_mapping[old_name])?;
        // Parse the label from the global string (format: USER:RESULT:tag)
        let old_label = full_tag
            .rfind(':')
            .and_then(|pos| pos.checked_add(1))
            .map_or_else(|| full_tag.clone(), |pos| full_tag[pos..].to_string());

        let (new_const, new_name) = build_result_global(ctx, &old_label, old_name, type_tag)?;

        let new_global = module.add_global(new_const.get_type(), None, &new_name);
        new_global.set_initializer(&new_const);
        new_global.set_linkage(inkwell::module::Linkage::Private);
        new_global.set_constant(true);
        global_mapping.insert(old_name.to_string(), new_global);
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

/// Core QIR to QIS translation logic without `PyO3` dependencies.
///
/// This function can be imported by other crates that need to use the translation
/// logic without depending on `PyO3`.
///
/// # Arguments
/// - `bc_bytes` - The QIR bytes to translate.
/// - `opt_level` - The optimization level to use (0-3).
/// - `target` - Target architecture ("aarch64", "x86-64", "native").
/// - `wasm_bytes` - Optional WASM bytes for Wasm codegen.
///
/// # Errors
/// Returns an error string if the translation fails.
pub fn qir_to_qis_core(
    bc_bytes: &[u8],
    opt_level: u32,
    target: &str,
    _wasm_bytes: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    use crate::{
        aux::process_entry_function,
        convert::{
            add_qmain_wrapper, create_qubit_array, find_entry_function, free_all_qubits,
            get_string_attrs, process_ir_defined_q_fns,
        },
        decompose::add_decompositions,
        opt::optimize,
        utils::add_generator_metadata,
    };
    use inkwell::{attributes::AttributeLoc, context::Context, memory_buffer::MemoryBuffer};
    use std::{collections::BTreeMap, env};

    let ctx = Context::create();
    let memory_buffer = MemoryBuffer::create_from_memory_range(bc_bytes, "bitcode");
    let module = ctx
        .create_module_from_ir(memory_buffer)
        .map_err(|e| format!("Failed to create module: {e}"))?;

    let _ = add_decompositions(&ctx, &module);

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

    // No channel management in core: external calls are generic and stateless

    free_all_qubits(&ctx, &module, entry_fn, qubit_array)?;

    // Add qmain wrapper that calls setup, entry function, and teardown
    let _ = add_qmain_wrapper(&ctx, &module, entry_fn);

    module
        .verify()
        .map_err(|e| format!("LLVM module verification failed: {e}"))?;

    optimize(&module, opt_level, target)?;

    // Clean up the translated module
    for attr in get_string_attrs(entry_fn) {
        let kind_id = attr
            .get_string_kind_id()
            .to_str()
            .map_err(|e| format!("Invalid UTF-8 in attribute kind ID: {e}"))?;
        entry_fn.remove_string_attribute(AttributeLoc::Function, kind_id);
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

    Ok(module.write_bitcode_to_memory().as_slice().to_vec())
}

// Core Rust API functions (without PyO3 dependencies)

#[cfg(feature = "wasm")]
fn get_wasm_functions(
    wasm_bytes: Option<&[u8]>,
    _errors: &mut Vec<String>,
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
    _errors: &mut Vec<String>,
) -> Result<std::collections::BTreeMap<String, u64>, String> {
    Ok(std::collections::BTreeMap::new())
}

/// Validate the given QIR bitcode.
///
/// This is the core validation logic that can be used without `PyO3` dependencies.
///
/// # Arguments
/// - `bc_bytes` - The QIR bytes to validate.
/// - `wasm_bytes` - Optional WASM bytes to validate against.
///
/// # Errors
/// Returns an error string if validation fails.
pub fn validate_qir_core(bc_bytes: &[u8], wasm_bytes: Option<&[u8]>) -> Result<(), String> {
    use crate::{
        aux::{validate_functions, validate_module_flags, validate_module_layout_and_triple},
        convert::find_entry_function,
    };
    use inkwell::{attributes::AttributeLoc, context::Context, memory_buffer::MemoryBuffer};

    let ctx = Context::create();
    let memory_buffer = MemoryBuffer::create_from_memory_range(bc_bytes, "bitcode");
    let module = ctx
        .create_module_from_ir(memory_buffer)
        .map_err(|e| format!("Failed to parse bitcode: {e}"))?;
    let mut errors = Vec::new();

    validate_module_layout_and_triple(&module);

    let entry_fn = if let Ok(entry_fn) = find_entry_function(&module) {
        if entry_fn.get_basic_blocks().is_empty() {
            errors.push("Entry function has no basic blocks".to_string());
        }

        // Enforce required attributes
        let required_attrs = [
            "required_num_qubits",
            "required_num_results",
            "qir_profiles",
            "output_labeling_schema",
        ];
        for &attr in &required_attrs {
            let val = entry_fn.get_string_attribute(AttributeLoc::Function, attr);
            if val.is_none() {
                errors.push(format!("Missing required attribute: `{attr}`"));
            }
        }

        // Check values for non-zero qubits/results
        for (attr, type_) in [
            ("required_num_qubits", "qubit"),
            ("required_num_results", "result"),
        ] {
            if entry_fn
                .get_string_attribute(AttributeLoc::Function, attr)
                .and_then(|a| a.get_string_value().to_str().ok()?.parse::<u32>().ok())
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

    let wasm_fns = get_wasm_functions(wasm_bytes, &mut errors)?;

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
/// This is the core conversion logic that can be used without `PyO3` dependencies.
///
/// # Errors
/// Returns an error string if the LLVM IR is invalid.
pub fn qir_ll_to_bc_core(ll_text: &str) -> Result<Vec<u8>, String> {
    use inkwell::{context::Context, memory_buffer::MemoryBuffer};

    let ctx = Context::create();
    let memory_buffer = MemoryBuffer::create_from_memory_range(ll_text.as_bytes(), "qir");
    let module = ctx
        .create_module_from_ir(memory_buffer)
        .map_err(|e| format!("Failed to create module from LLVM IR: {e}"))?;

    Ok(module.write_bitcode_to_memory().as_slice().to_vec())
}

/// Get QIR entry point function attributes.
///
/// These attributes are used to generate METADATA records in QIR output schemas.
/// This function assumes that QIR has been validated using `validate_qir_core`.
///
/// # Errors
/// Returns an error string if the input bitcode is invalid.
pub fn get_entry_attributes_core(
    bc_bytes: &[u8],
) -> Result<std::collections::BTreeMap<String, Option<String>>, String> {
    use crate::convert::{find_entry_function, get_string_attrs};
    use inkwell::{context::Context, memory_buffer::MemoryBuffer};
    use std::collections::BTreeMap;

    let ctx = Context::create();
    let memory_buffer = MemoryBuffer::create_from_memory_range(bc_bytes, "bitcode");
    let module = ctx
        .create_module_from_ir(memory_buffer)
        .map_err(|e| format!("Failed to create module from QIR bitcode: {e}"))?;

    let mut metadata = BTreeMap::new();
    if let Ok(entry_fn) = find_entry_function(&module) {
        for attr in get_string_attrs(entry_fn) {
            let kind_id = if let Ok(kind_id) = attr.get_string_kind_id().to_str() {
                kind_id.to_owned()
            } else {
                log::warn!("Skipping invalid UTF-8 attribute kind ID");
                continue;
            };
            if let Ok(value) = attr.get_string_value().to_str() {
                metadata.insert(
                    kind_id,
                    if value.is_empty() {
                        None
                    } else {
                        Some(value.to_owned())
                    },
                );
            } else {
                log::warn!("Invalid UTF-8 value for attribute `{kind_id}`");
                metadata.insert(kind_id, None);
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
pub mod qir_qis {
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
    #[pyo3(signature = (bc_bytes, wasm_bytes = None))]
    pub fn validate_qir(bc_bytes: Cow<[u8]>, wasm_bytes: Option<Cow<[u8]>>) -> PyResult<()> {
        crate::validate_qir_core(&bc_bytes, wasm_bytes.as_deref())
            .map_err(PyErr::new::<ValidationError, _>)
    }

    /// Translate QIR bitcode to Quantinuum QIS.
    ///
    /// # Arguments
    /// - `bc_bytes` - The QIR bytes to translate.
    /// - `opt_level` - The optimization level to use (0-3). Default is 2.
    /// - `target` - Target architecture (default: "aarch64"; options: "x86-64", "native").
    /// - `wasm_bytes` - Optional WASM bytes for Wasm codegen.
    ///
    /// # Errors
    /// Returns a `CompilerError` if the translation fails.
    #[gen_stub_pyfunction]
    #[pyfunction]
    #[allow(clippy::needless_pass_by_value)]
    #[allow(clippy::missing_errors_doc)]
    #[pyo3(signature = (bc_bytes, *, opt_level = 2, target = "aarch64", wasm_bytes = None))]
    pub fn qir_to_qis<'a>(
        bc_bytes: Cow<[u8]>,
        opt_level: u32,
        target: &'a str,
        wasm_bytes: Option<Cow<'a, [u8]>>,
    ) -> PyResult<Cow<'a, [u8]>> {
        let result = crate::qir_to_qis_core(&bc_bytes, opt_level, target, wasm_bytes.as_deref())
            .map_err(PyErr::new::<CompilerError, _>)?;

        Ok(result.into())
    }

    /// Convert QIR LLVM IR to QIR bitcode.
    ///
    /// # Errors
    /// Returns a `ValidationError` if the LLVM IR is invalid.
    #[gen_stub_pyfunction]
    #[pyfunction]
    pub fn qir_ll_to_bc(ll_text: &str) -> PyResult<Cow<'_, [u8]>> {
        let result = crate::qir_ll_to_bc_core(ll_text).map_err(PyErr::new::<ValidationError, _>)?;
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
    pub fn get_entry_attributes(bc_bytes: Cow<[u8]>) -> PyResult<BTreeMap<String, Option<String>>> {
        crate::get_entry_attributes_core(&bc_bytes).map_err(PyErr::new::<ValidationError, _>)
    }
}

#[cfg(feature = "python")]
define_stub_info_gatherer!(stub_info);

#[cfg(test)]
mod test {
    #![allow(clippy::expect_used)]
    #![allow(clippy::unwrap_used)]
    use super::qir_qis::*;

    #[test]
    fn test_get_entry_attributes() {
        let ll_text =
            std::fs::read_to_string("tests/data/base-attrs.ll").expect("Failed to read base.ll");
        let bc_bytes = qir_ll_to_bc(&ll_text).unwrap();
        let attrs = get_entry_attributes(bc_bytes).unwrap();
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
}
