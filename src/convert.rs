use ::std::hash::BuildHasher;
use std::collections::HashMap;
use std::convert::Into;
use std::error::Error;

use inkwell::AddressSpace;
use inkwell::attributes::{Attribute, AttributeLoc};
use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{AnyTypeEnum, FunctionType};
use inkwell::values::{
    AnyValue, ArrayValue, AsValueRef, BasicValue, BasicValueEnum, CallSiteValue, FunctionValue,
    GlobalValue, InstructionOpcode, InstructionValue, PointerValue,
};
use llvm_sys::core::{
    LLVMGetAsString, LLVMGetNumOperands, LLVMGetOperand, LLVMGetValueName2, LLVMIsConstantString,
};
use llvm_sys::{
    LLVMAttributeFunctionIndex,
    core::{LLVMGetAttributeCountAtIndex, LLVMGetAttributesAtIndex, LLVMIsStringAttribute},
    prelude::LLVMAttributeRef,
};

pub const INIT_QARRAY_FN: &str = "qir_qis.init_qubit";
pub const LOAD_QUBIT_FN: &str = "qir_qis.load_qubit";
pub const ENTRY_ATTRIBUTE_KEYS: [&str; 5] = [
    "entry_point",
    "qir_profiles",
    "output_labeling_schema",
    "required_num_qubits",
    "required_num_results",
];
const EXIT_CODE: u64 = 1001;
const RESULT_TAG: &str = "USER";

/// Checks if the given type is an i8 array type.
fn is_i8_array_type(ty: AnyTypeEnum) -> bool {
    ty.is_array_type()
        && ty
            .into_array_type()
            .get_element_type()
            .into_int_type()
            .get_bit_width()
            == 8
}

/// Processes a global variable by replacing its initializer with a
/// length- and "USER:RESULT:" prefixed string. It also creates a new global
/// variable with a "res_" prefixed name.
fn translate_global<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    old_global: GlobalValue,
    global_mapping: &mut HashMap<String, GlobalValue<'ctx>>,
    empty_tag_counter: &mut usize,
) -> Result<(), String> {
    // 1. Extract original string
    let label = get_string_label(old_global)?;

    // 2. Create new string with prefix
    let old_global_name = old_global
        .get_name()
        .to_str()
        .map_err(|e| format!("Invalid UTF-8 in global name: {e}"))?
        .to_string();
    let old_global_key = if old_global_name.is_empty() {
        label.clone()
    } else {
        old_global_name.clone()
    };
    let empty_tag_index = if label.is_empty() {
        let idx = *empty_tag_counter;
        *empty_tag_counter = empty_tag_counter.saturating_add(1);
        Some(idx)
    } else {
        None
    };
    let (new_const, new_name) =
        build_result_global(context, &label, &old_global_name, "RESULT", empty_tag_index)?;
    let new_global = module.add_global(new_const.get_type(), None, &new_name);
    new_global.set_initializer(&new_const);
    new_global.set_linkage(Linkage::Private);
    new_global.set_constant(true);

    global_mapping.insert(old_global_key, new_global);
    Ok(())
}

/// Extracts the string label from a global variable's initializer.
///
/// # Errors
/// Returns an error if the global variable has no initializer or if the initializer is not a constant string.
pub fn get_string_label(old_global: GlobalValue<'_>) -> Result<String, String> {
    let init = old_global
        .get_initializer()
        .ok_or("Global has no initializer")?;
    if init.get_type().is_array_type() && init.get_type().into_array_type().len() == 1 {
        return Ok(String::new());
    }
    let init_ref = init.as_value_ref();
    let is_const_str = unsafe { LLVMIsConstantString(init_ref) } != 0;
    if !is_const_str {
        return Err("Global initializer is not a constant string".to_string());
    }

    let mut len: usize = 0;
    let ptr = unsafe { LLVMGetAsString(init_ref, &raw mut len) };
    if ptr.is_null() {
        return Err("LLVMGetAsString returned null pointer".to_string());
    }

    let raw = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    let until_nul = raw.split(|b| *b == 0).next().unwrap_or(raw);
    String::from_utf8(until_nul.to_vec())
        .map_err(|e| format!("Invalid UTF-8 in global initializer: {e}"))
}

/// Constructs a new global string constant and name for a classical result.
///
/// Prepends the "TAG:" and "TYPE:" to the original string, adds a length prefix,
/// and returns the new LLVM constant and the new global variable name.
///
/// # Errors
/// Returns an error if the length of the new string exceeds 255 bytes.
pub fn build_result_global<'a>(
    context: &'a Context,
    label: &str,
    _old_name: &str,
    ty: &str,
    empty_tag_index: Option<usize>,
) -> Result<(ArrayValue<'a>, String), String> {
    let new_cl_str_bytes = create_cl_str(RESULT_TAG, ty, label)?;
    let new_const = context.const_string(&new_cl_str_bytes, false);

    let new_name = if label.is_empty() {
        empty_tag_index.map_or_else(
            || "res_empty_tag".to_string(),
            |idx| format!("res_empty_tag.{idx}"),
        )
    } else {
        format!("res_{}", sanitize_label_for_global_name(label))
    };
    Ok((new_const, new_name))
}

fn sanitize_label_for_global_name(label: &str) -> String {
    label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Creates a CL string in the format "TAG:TYPE:LABEL" with a length prefix.
fn create_cl_str(tag: &str, ty: &str, label: &str) -> Result<Vec<u8>, String> {
    let new_str = format!("{tag}:{ty}:{label}");
    let new_bytes = new_str.as_bytes();
    let new_len = new_bytes.len();
    if new_len >= 256 {
        return Err(format!("Constant string too long: {new_len} >= 256"));
    }
    let new_cl_str_bytes = [
        &[u8::try_from(new_len)
            .map_err(|_| format!("Failed to convert length {new_len} to u8"))?],
        new_bytes,
    ]
    .concat();
    Ok(new_cl_str_bytes)
}

/// Converts global variables in the module to the QIS format.
///
/// # Errors
/// Returns an error if the conversion fails.
pub fn convert_globals<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
) -> Result<HashMap<String, GlobalValue<'ctx>>, String> {
    // Collect globals matching the pattern
    let old_globals: Vec<_> = module
        .get_globals()
        .filter(|g| {
            (g.get_linkage() == Linkage::Internal || g.get_linkage() == Linkage::Private)
                && g.is_constant()
                && is_i8_array_type(g.get_value_type())
        })
        .collect();

    let mut global_mapping: HashMap<String, GlobalValue> = HashMap::new();
    let mut empty_tag_counter: usize = 0;
    for old_global in old_globals {
        translate_global(
            context,
            module,
            old_global,
            &mut global_mapping,
            &mut empty_tag_counter,
        )?;
    }
    Ok(global_mapping)
}

/// Finds the entry function in the module.
/// The entry function is identified by the presence of the `entry_point` attribute.
///
/// # Errors
/// Returns an error if no entry function is found.
pub fn find_entry_function<'a>(module: &Module<'a>) -> Result<FunctionValue<'a>, String> {
    for function in module.get_functions() {
        if function
            .get_string_attribute(AttributeLoc::Function, "entry_point")
            .is_some()
        {
            return Ok(function);
        }
    }
    Err("No entry function found".to_string())
}

/// Retrieves all string attributes from a function.
#[must_use]
pub fn get_string_attrs(function: FunctionValue) -> Vec<Attribute> {
    let count = usize::try_from(unsafe {
        LLVMGetAttributeCountAtIndex(function.as_value_ref(), LLVMAttributeFunctionIndex)
    })
    .unwrap_or(0);
    if count == 0 {
        return Vec::new();
    }

    let mut attrs: Vec<LLVMAttributeRef> = vec![std::ptr::null_mut(); count];
    unsafe {
        LLVMGetAttributesAtIndex(
            function.as_value_ref(),
            LLVMAttributeFunctionIndex,
            attrs.as_mut_ptr(),
        );
    }

    attrs
        .iter()
        .copied()
        .filter(|attr| !attr.is_null())
        .filter(|attr| unsafe { LLVMIsStringAttribute(*attr) } != 0)
        .map(|attr| unsafe { Attribute::new(attr) })
        .collect()
}

/// Extracts the `required_num_qubits` attribute from a function.
///
/// # Returns
/// `Some(num_qubits)` if the attribute exists and can be parsed as a `u32`, otherwise `None`.
#[must_use]
pub fn get_required_num_qubits(function: FunctionValue) -> Option<u32> {
    function
        .get_string_attribute(AttributeLoc::Function, "required_num_qubits")
        .and_then(|attr| attr.get_string_value().to_str().ok()?.parse::<u32>().ok())
}

/// Extracts and validates the `required_num_qubits` attribute from a function.
///
/// # Errors
/// Returns an error when the attribute is missing or cannot be parsed as `u32`.
pub fn get_required_num_qubits_strict(function: FunctionValue) -> Result<u32, String> {
    let attr = function
        .get_string_attribute(AttributeLoc::Function, "required_num_qubits")
        .ok_or("Missing or invalid required_num_qubits attribute")?;
    let raw = attr
        .get_string_value()
        .to_str()
        .map_err(|_| "Invalid required_num_qubits attribute (not UTF-8)".to_string())?;
    raw.parse::<u32>()
        .map_err(|_| format!("Invalid required_num_qubits attribute value: {raw}"))
}

/// Creates a global array of qubits and initializes them using `___qalloc` calls.
/// The array is filled with qubit handles, and each qubit is reset using `___reset`.
///
/// # Returns
/// A pointer to the global qubit array.
/// # Errors
/// Returns an error if the creation fails.
pub fn create_qubit_array<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    entry_fn: FunctionValue,
) -> Result<PointerValue<'ctx>, String> {
    // 1. Extract `required_num_qubits` from function attributes
    let num_qubits = get_required_num_qubits_strict(entry_fn)?;

    // 2. Create a global static array with dummy initializer
    let i64_type = ctx.i64_type();
    let array_type = i64_type.array_type(num_qubits);
    let dummy_init = i64_type.const_array(&vec![i64_type.const_zero(); num_qubits as usize]);
    let global_qubits = module.add_global(array_type, None, "qis_qs");
    global_qubits.set_initializer(&dummy_init);
    global_qubits.set_linkage(Linkage::Private);
    global_qubits.set_constant(false);

    // 3. Fill the global array with qalloc calls in the entry block
    let builder = ctx.create_builder();
    let entry_block = entry_fn
        .get_first_basic_block()
        .ok_or("Entry function has no basic blocks")?;
    builder.position_before(
        &entry_block
            .get_first_instruction()
            .ok_or("Entry block has no instructions")?,
    );

    let global_ptr = global_qubits.as_pointer_value();
    let init_qbits_fn = add_init_qubit_fn(ctx, module, global_ptr, array_type).and_then(|()| {
        module
            .get_function(INIT_QARRAY_FN)
            .ok_or_else(|| format!("{INIT_QARRAY_FN} function not found"))
    })?;
    for i in 0..num_qubits {
        let index_val = i64_type.const_int(u64::from(i), false);
        builder
            .build_call(init_qbits_fn, &[index_val.into()], "")
            .map_err(|e| format!("Failed to build call to {INIT_QARRAY_FN}: {e}"))?;
    }

    let _ = add_load_qubit_fn(ctx, module, global_ptr, array_type);

    Ok(global_ptr)
}

/// Builds a function to load a qubit from the global qubit array.
///
/// It derives the index at runtime by subtracting the null pointer from the given pointer,
/// then converting the difference to an integer.
fn add_load_qubit_fn<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    global_ptr: PointerValue<'_>,
    qubit_array_type: inkwell::types::ArrayType<'ctx>,
) -> Result<(), String> {
    let i64_type = ctx.i64_type();
    let qubit_ptr_type = ctx.ptr_type(AddressSpace::default());
    // i64 @qir_qis.load_qubit(ptr %q)
    let fn_type = i64_type.fn_type(&[qubit_ptr_type.into()], false);
    let function = module.add_function(LOAD_QUBIT_FN, fn_type, None);

    let entry = ctx.append_basic_block(function, "entry");
    let builder = ctx.create_builder();
    builder.position_at_end(entry);

    let qubit_ptr = function
        .get_first_param()
        .ok_or("Function has no parameters")?
        .into_pointer_value();

    let index_val = builder
        .build_ptr_to_int(qubit_ptr, i64_type, "idx")
        .map_err(|e| format!("Failed to build ptr_to_int: {e}"))?;

    let elem_ptr = unsafe {
        builder
            .build_gep(
                qubit_array_type,
                global_ptr,
                &[i64_type.const_zero(), index_val],
                "qbit_ptr",
            )
            .map_err(|e| format!("Failed to build GEP: {e}"))?
    };

    let handle = build_load_qbit(ctx, &builder, elem_ptr)?;
    builder
        .build_return(Some(&handle))
        .map_err(|e| format!("Failed to build return: {e}"))?;
    Ok(())
}

/// Adds a function to initialize a single qubit in the global array.
/// This function allocates, resets, and stores a qubit at a given index.
fn add_init_qubit_fn<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    global_ptr: PointerValue<'ctx>,
    qubit_array_type: inkwell::types::ArrayType<'ctx>,
) -> Result<(), String> {
    let i64_type = ctx.i64_type();
    // void @qir_qis.init_qubit(i64 %index)
    let fn_type = ctx.void_type().fn_type(&[i64_type.into()], false);
    let init_qbits_fn = module.add_function(INIT_QARRAY_FN, fn_type, Some(Linkage::Private));

    let entry = ctx.append_basic_block(init_qbits_fn, "entry");
    let builder = ctx.create_builder();
    builder.position_at_end(entry);

    let index = init_qbits_fn
        .get_first_param()
        .ok_or("Function has no parameters")?
        .into_int_value();

    let fn_type = ctx.i64_type().fn_type(&[], false);
    let qalloc_fn = get_or_create_function(module, "___qalloc", fn_type);

    // Call ___qalloc
    let call_result = builder
        .build_call(qalloc_fn, &[], "qalloc")
        .map_err(|e| format!("Failed to build qalloc call: {e}"))?;
    let qid = match call_result.try_as_basic_value() {
        inkwell::values::ValueKind::Basic(bv) => bv,
        inkwell::values::ValueKind::Instruction(_) => {
            return Err("qalloc call did not return a basic value".into());
        }
    };

    process_allocation_error(ctx, module, &builder, qid)?;

    // Call ___reset
    create_reset_call(ctx, module, &builder, qid);

    // Store to global array
    let ptr = unsafe {
        builder
            .build_gep(
                qubit_array_type,
                global_ptr,
                &[i64_type.const_zero(), index],
                "qubit_ptr",
            )
            .map_err(|e| format!("Failed to build GEP for qubit: {e}"))?
    };
    builder
        .build_store(ptr, qid)
        .map_err(|e| format!("Failed to store qubit: {e}"))?;
    builder
        .build_return(None)
        .map_err(|e| format!("Failed to build return: {e}"))?;

    Ok(())
}

/// Processes a qubit allocation error by checking if the returned qubit ID
/// is equal to `u64::MAX`. If so, it calls a panic function with an error message
/// and exits the program.
fn process_allocation_error<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder<'_>,
    qid: BasicValueEnum<'_>,
) -> Result<(), String> {
    let fail_val = ctx.i64_type().const_int(u64::MAX, false);
    let is_fail = builder
        .build_int_compare(
            inkwell::IntPredicate::EQ,
            qid.into_int_value(),
            fail_val,
            "is_fail",
        )
        .map_err(|e| format!("Failed to build int compare: {e}"))?;
    let parent = builder
        .get_insert_block()
        .and_then(BasicBlock::get_parent)
        .ok_or("No parent function for block")?;
    let fail_block = ctx.append_basic_block(parent, "qalloc_fail");
    let cont_block = ctx.append_basic_block(parent, "qalloc_ok");
    builder
        .build_conditional_branch(is_fail, fail_block, cont_block)
        .map_err(|e| format!("Failed to build conditional branch: {e}"))?;
    builder.position_at_end(fail_block);
    let msg_bytes = create_cl_str("EXIT", "INT", "No more qubits available to allocate.")?;
    let arr_ty = ctx.i8_type().array_type(
        u32::try_from(msg_bytes.len()).map_err(|e| format!("Failed to create array type: {e}"))?,
    );
    let msg_const = ctx.const_string(&msg_bytes, false);
    let global_name = "e_qalloc_fail";
    let err_global = module.get_global(global_name).unwrap_or_else(|| {
        let g = module.add_global(arr_ty, None, global_name);
        g.set_initializer(&msg_const);
        g.set_linkage(Linkage::Private);
        g.set_constant(true);
        g
    });
    let gep = unsafe {
        builder
            .build_gep(
                arr_ty,
                err_global.as_pointer_value(),
                &[ctx.i64_type().const_zero(), ctx.i64_type().const_zero()],
                "err_gep",
            )
            .map_err(|e| format!("Failed to build GEP for panic message: {e}"))?
    };
    let fn_type = ctx.void_type().fn_type(
        &[
            ctx.i32_type().into(),
            ctx.ptr_type(AddressSpace::default()).into(),
        ],
        false,
    );
    let panic_fn = get_or_create_function(module, "panic", fn_type);
    let _ = builder
        .build_call(
            panic_fn,
            &[
                ctx.i32_type().const_int(EXIT_CODE, false).into(),
                gep.into(),
            ],
            "",
        )
        .map_err(|e| format!("Failed to build panic call: {e}"))?;
    builder
        .build_unreachable()
        .map_err(|e| format!("Failed to build unreachable: {e}"))?;
    builder.position_at_end(cont_block);
    Ok(())
}

/// Builds a load instruction for a qubit from the given pointer.
fn build_load_qbit<'a>(
    ctx: &'a Context,
    builder: &Builder<'a>,
    elem_ptr: PointerValue<'a>,
) -> Result<BasicValueEnum<'a>, String> {
    builder
        .build_load(ctx.i64_type(), elem_ptr, "qbit")
        .map_err(|e| format!("Failed to build load instruction: {e}"))
}

/// Retrieves the result SSA variables from the entry function.
/// The result variable count is equal to the `required_num_results` attribute.
///
/// # Returns
/// a vector of `Option<(BasicValueEnum, Option<BasicValueEnum>)>`.
/// Each element in the vector corresponds to a result variable, where:
/// - The first element stores the measurement future (if any).
/// - The second element is an optional value that can be used to store the SSA
///   variable for the result future after it is read.
///
/// # Errors
/// Returns an error if the `required_num_results` attribute is missing or invalid.
#[allow(clippy::type_complexity)]
pub fn get_result_vars(
    entry_fn: FunctionValue,
) -> Result<Vec<Option<(BasicValueEnum, Option<BasicValueEnum>)>>, String> {
    let num_results = entry_fn
        .get_string_attribute(AttributeLoc::Function, "required_num_results")
        .and_then(|attr| attr.get_string_value().to_str().ok()?.parse::<u32>().ok())
        .ok_or("Missing required_num_results")?;

    Ok(vec![None; num_results as usize])
}

/// Frees all qubits in the qubit array.
///
/// # Errors
/// Returns an error if the qubit array is invalid.
pub fn free_all_qubits<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    entry_fn: FunctionValue,
    qubit_array: PointerValue,
) -> Result<(), String> {
    let builder = ctx.create_builder();
    for bb in entry_fn.get_basic_blocks() {
        for instr in bb.get_instructions() {
            if instr.get_opcode() == InstructionOpcode::Return {
                builder.position_before(&instr);
                let i64_type = ctx.i64_type();
                let num_qubits = get_required_num_qubits_strict(entry_fn)?;
                let qubit_array_type = i64_type.array_type(num_qubits);
                for i in 0..num_qubits {
                    let index_val = i64_type.const_int(u64::from(i), false);
                    let elem_ptr = unsafe {
                        builder.build_gep(
                            qubit_array_type,
                            qubit_array,
                            &[i64_type.const_zero(), index_val],
                            "qbit",
                        )
                    };
                    let elem_ptr = match elem_ptr {
                        Ok(ptr) => ptr,
                        Err(e) => return Err(format!("Failed to build GEP for qubit {i}: {e}")),
                    };
                    let q_handle = build_load_qbit(ctx, &builder, elem_ptr)?;
                    create_qfree_call(ctx, module, &builder, q_handle);
                }
            }
        }
    }
    Ok(())
}

/// Adds the wrapper function `qmain` to the module.
///
/// # Errors
/// Returns an error if the wrapper function cannot be added.
pub fn add_qmain_wrapper<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    entry_fn: FunctionValue<'ctx>,
) -> Result<(), String> {
    // 1. Declare qmain function type: i64 (i64)
    let i64_type = context.i64_type();
    let qmain_type = i64_type.fn_type(&[i64_type.into()], false);
    let qmain_fn = get_or_create_function(module, "qmain", qmain_type);

    // 2. Create entry block
    let entry_block = context.append_basic_block(qmain_fn, "entry");
    let builder = context.create_builder();
    builder.position_at_end(entry_block);

    // 3. Declare setup/teardown functions
    let setup_fn = get_or_create_function(
        module,
        "setup",
        context.void_type().fn_type(&[i64_type.into()], false),
    );

    let teardown_fn = get_or_create_function(module, "teardown", i64_type.fn_type(&[], false));

    // 4. Get qmain parameter
    let arg0 = qmain_fn
        .get_first_param()
        .ok_or("qmain function has no parameters")?
        .into_int_value();

    // 5. Build function calls
    let _ = builder.build_call(setup_fn, &[arg0.into()], "");

    let _ = builder.build_call(entry_fn, &[], "");

    let teardown_call = builder
        .build_call(teardown_fn, &[], "retval")
        .map_err(|e| format!("Failed to build teardown call: {e}"))?;

    // 6. Return teardown result
    let ret_val = match teardown_call.try_as_basic_value() {
        inkwell::values::ValueKind::Basic(bv) => bv,
        inkwell::values::ValueKind::Instruction(_) => {
            return Err("Teardown call did not return a basic value".into());
        }
    };
    let _ = builder.build_return(Some(&ret_val));

    Ok(())
}

/// Helper to replace a native quantum call with a new function and mapped arguments.
///
/// # Arguments
/// - `context`: The LLVM context.
/// - `module`: The LLVM module.
/// - `old_call`: The instruction value of the old call.
/// - `new_fn_name`: The name of the new function to call.
/// - `arg_types`: The types of the new function's arguments.
/// - `arg_map`: A closure that maps the old call's arguments to the new call's arguments.
///
/// # Errors
/// Returns an error if the replacement fails.
fn replace_native_call<'ctx, F>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    old_call: InstructionValue<'ctx>,
    gate_name: &str,
    arg_types: &[inkwell::types::BasicTypeEnum<'ctx>],
    arg_map: F,
) -> Result<(), Box<dyn Error>>
where
    F: for<'a> Fn(
        &[BasicValueEnum<'ctx>],
        &'a Builder<'ctx>,
    ) -> Result<Vec<BasicValueEnum<'a>>, String>,
{
    let builder = context.create_builder();
    builder.position_before(&old_call);

    let args: Result<Vec<BasicValueEnum>, String> = (0..old_call.get_num_operands())
        .map(|i| {
            let op = old_call
                .get_operand(i)
                .ok_or_else(|| format!("Operand {i} not found"))?;
            match op {
                inkwell::values::Operand::Value(bv) => Ok(bv),
                inkwell::values::Operand::Block(_) => {
                    Err(format!("Operand {i} is not a BasicValueEnum"))
                }
            }
        })
        .collect();

    let args = match args {
        Ok(a) => a,
        Err(e) => {
            log::error!("Error collecting call operands: {e}");
            return Err(Box::<dyn Error>::from(e));
        }
    };

    let mapped_args = arg_map(&args, &builder).map_err(|e| {
        log::error!("Error mapping arguments: {e}");
        e
    })?;

    let arg_metadata_types: Vec<_> = arg_types.iter().map(|t| (*t).into()).collect();
    let fn_type = context.void_type().fn_type(&arg_metadata_types, false);
    let func = get_or_create_function(module, gate_name, fn_type);

    let args_into: Vec<_> = mapped_args.into_iter().map(Into::into).collect();
    let _ = builder.build_call(func, &args_into, "");

    old_call.erase_from_basic_block();
    Ok(())
}

/// Replaces a call to `__quantum__qis__rxy__body` with a call to `___rxy`.
///
/// # Errors
/// Returns an error if the replacement fails.
pub fn replace_rxy_call<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    old_call: InstructionValue<'a>,
) -> Result<(), String> {
    let get_idx_fn = module
        .get_function(LOAD_QUBIT_FN)
        .ok_or_else(|| format!("{LOAD_QUBIT_FN} not found"))?;

    let _ = replace_native_call(
        ctx,
        module,
        old_call,
        "___rxy",
        &[
            ctx.i64_type().into(), // qubit handle
            ctx.f64_type().into(), // angle
            ctx.f64_type().into(), // angle
        ],
        |args, builder| {
            let qubit_ptr = args[2].into_pointer_value();
            let idx_call = builder
                .build_call(get_idx_fn, &[qubit_ptr.into()], "qbit")
                .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}"))?;
            let handle = match idx_call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err(format!("{LOAD_QUBIT_FN} did not return a basic value"));
                }
            };
            Ok(vec![handle, args[0], args[1]])
        },
    );
    Ok(())
}

/// Replaces a call to `__quantum__qis__rz__body` with a call to `___rz`.
///
/// # Errors
/// Returns an error if the replacement fails.
pub fn replace_rz_call<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    old_call: InstructionValue<'a>,
) -> Result<(), String> {
    let get_idx_fn = module
        .get_function(LOAD_QUBIT_FN)
        .ok_or_else(|| format!("{LOAD_QUBIT_FN} not found"))?;
    let _ = replace_native_call(
        ctx,
        module,
        old_call,
        "___rz",
        &[ctx.i64_type().into(), ctx.f64_type().into()],
        |args, builder| {
            let qubit_ptr = args[1].into_pointer_value();
            let idx_call = builder
                .build_call(get_idx_fn, &[qubit_ptr.into()], "qbit")
                .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}"))?;
            let handle = match idx_call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err(format!("{LOAD_QUBIT_FN} did not return a basic value"));
                }
            };
            Ok(vec![handle, args[0]])
        },
    );
    Ok(())
}

/// Replaces a call to `__quantum__qis__rzz__body` with a call to `___rzz`.
///
/// # Errors
/// Returns an error if the replacement fails.
pub fn replace_rzz_call<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    old_call: InstructionValue<'a>,
) -> Result<(), String> {
    let get_idx_fn = module
        .get_function(LOAD_QUBIT_FN)
        .ok_or_else(|| format!("{LOAD_QUBIT_FN} not found"))?;
    let _ = replace_native_call(
        ctx,
        module,
        old_call,
        "___rzz",
        &[
            ctx.i64_type().into(), // qubit handle
            ctx.i64_type().into(), // qubit handle
            ctx.f64_type().into(), // angle
        ],
        |args, builder| {
            let qubit_ptr = args[1].into_pointer_value();
            let idx_call = builder
                .build_call(get_idx_fn, &[qubit_ptr.into()], "qbit")
                .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}"))?;
            let q1 = match idx_call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err(format!("{LOAD_QUBIT_FN} did not return a basic value"));
                }
            };
            let qubit_ptr = args[2].into_pointer_value();
            let idx_call = builder
                .build_call(get_idx_fn, &[qubit_ptr.into()], "qbit")
                .map_err(|e| format!("Failed to build call to {LOAD_QUBIT_FN}: {e}"))?;
            let q2 = match idx_call.try_as_basic_value() {
                inkwell::values::ValueKind::Basic(bv) => bv,
                inkwell::values::ValueKind::Instruction(_) => {
                    return Err(format!("{LOAD_QUBIT_FN} did not return a basic value"));
                }
            };
            Ok(vec![q1, q2, args[0]])
        },
    );
    Ok(())
}

/// Extracts the qubit index from an `IntToPtr` conversion string.
fn get_idx_from_pointer_repr(ir_string: &str) -> Result<u64, String> {
    // Expected form: `inttoptr (i64 <index> to ...)`
    let pattern = "inttoptr (i64 ";
    if let Some(start) = ir_string.find(pattern) {
        let rest = &ir_string[start
            .checked_add(pattern.len())
            .ok_or("Failed to calculate string index")?..];
        if let Some(end) = rest.find(' ') {
            let num_str = &rest[..end];
            if let Ok(idx) = num_str.parse::<u64>() {
                return Ok(idx);
            }
        }
    }
    Err(format!("Cannot extract pointer index from: {ir_string}"))
}

/// Extracts the index from a pointer value.
/// Assumes the pointer is a result of an `IntToPtr` conversion.
///
/// # Errors
/// Returns an error if the index cannot be extracted.
pub fn get_index(arg: PointerValue) -> Result<u64, String> {
    if arg.is_null() {
        return Ok(0);
    }
    if arg.is_const() {
        let int_type = arg.get_type().get_context().i64_type();
        if let Some(idx) = arg.const_to_int(int_type).get_zero_extended_constant() {
            return Ok(idx);
        }
    }
    // Fallback: try to extract the index from an `inttoptr` representation.
    let ir_string = arg.print_to_string().to_string();
    get_idx_from_pointer_repr(&ir_string)
}

/// Creates a call to the `___qfree` function.
fn create_qfree_call<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    handle: BasicValueEnum,
) {
    let fn_type = context
        .void_type()
        .fn_type(&[context.i64_type().into()], false);
    let func = get_or_create_function(module, "___qfree", fn_type);
    let _ = builder.build_call(func, &[handle.into()], "");
}

/// Creates a call to the `___reset` function.
pub fn create_reset_call<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    handle: BasicValueEnum,
) {
    let fn_type = context
        .void_type()
        .fn_type(&[context.i64_type().into()], false);
    let func = get_or_create_function(module, "___reset", fn_type);
    let _ = builder.build_call(func, &[handle.into()], "");
}

/// Retrieves or creates a function in the module.
pub fn get_or_create_function<'ctx>(
    module: &Module<'ctx>,
    name: &str,
    fn_type: FunctionType<'ctx>,
) -> FunctionValue<'ctx> {
    module
        .get_function(name)
        .unwrap_or_else(|| module.add_function(name, fn_type, Some(Linkage::External)))
}

/// Records a classical output by replacing an instruction with a print call.
///
/// # Errors
/// Returns an error if the print call fails.
pub fn record_classical_output(
    ctx: &Context,
    instr: InstructionValue,
    new_global: GlobalValue,
    print_func: FunctionValue,
    val: BasicValueEnum,
) -> Result<(), String> {
    let builder = ctx.create_builder();
    builder.position_before(&instr);
    add_print_call(ctx, &builder, new_global, print_func, val)?;

    // Remove old call
    instr.erase_from_basic_block();
    Ok(())
}

/// Adds a print call to the module for the given print function and value.
///
/// # Errors
/// Returns an error if the print call fails.
pub fn add_print_call(
    ctx: &Context,
    builder: &Builder,
    new_global: GlobalValue,
    print_fn: FunctionValue,
    val: BasicValueEnum,
) -> Result<(), String> {
    // Create GEP for new global
    let zero = ctx.i64_type().const_zero();
    let global_array_type = new_global
        .get_initializer()
        .ok_or("Global has no initializer")?
        .into_array_value()
        .get_type();
    let gep = unsafe {
        builder
            .build_gep(
                global_array_type,
                new_global.as_pointer_value(),
                &[zero, zero],
                "gep",
            )
            .map_err(|e| format!("Failed to build GEP for print call: {e}"))?
    };

    let length = ctx.i64_type().const_int(
        u64::from(global_array_type.len())
            .checked_sub(1) // -1 to remove the length byte
            .ok_or("Array length must be at least 1")?,
        false,
    );

    builder
        .build_call(print_fn, &[gep.into(), length.into(), val.into()], "")
        .map_err(|e| format!("Failed to build print call: {e}"))?;
    Ok(())
}

/// Parses a `getelementptr` instruction to extract the global variable name.
///
/// # Errors
/// Returns an error if the GEP is not a pointer value.
pub fn parse_gep(gep: BasicValueEnum) -> Result<String, String> {
    match gep {
        BasicValueEnum::PointerValue(ptr) => {
            if let Some(instr) = ptr.as_instruction_value()
                && instr.get_opcode() == InstructionOpcode::GetElementPtr
            {
                let op0 = instr
                    .get_operand(0)
                    .ok_or("GEP instruction missing base operand")?;
                let inkwell::values::Operand::Value(base) = op0 else {
                    return Err("GEP base operand is not a value".to_string());
                };
                let base_ptr = base.into_pointer_value();
                let base_name = base_ptr
                    .get_name()
                    .to_str()
                    .map_err(|e| format!("Invalid UTF-8 in GEP base pointer name: {e}"))?;
                if !base_name.is_empty() {
                    return Ok(base_name.to_string());
                }
            }

            let ptr_name = ptr
                .get_name()
                .to_str()
                .map_err(|e| format!("Invalid UTF-8 in pointer name: {e}"))?;
            if !ptr_name.is_empty() {
                return Ok(ptr_name.to_string());
            }

            // Handle constant-expression GEPs by recursively walking operand values
            // via LLVM C APIs and finding either:
            // 1) a named underlying value, or
            // 2) a constant string label (for unnamed globals like @0, @1, ...).
            let mut stack = vec![ptr.as_value_ref()];
            while let Some(value_ref) = stack.pop() {
                let is_const_str = unsafe { LLVMIsConstantString(value_ref) } != 0;
                if is_const_str {
                    let mut len: usize = 0;
                    let str_ptr = unsafe { LLVMGetAsString(value_ref, &raw mut len) };
                    if !str_ptr.is_null() && len > 0 {
                        let raw = unsafe { std::slice::from_raw_parts(str_ptr.cast::<u8>(), len) };
                        let until_nul = raw.split(|b| *b == 0).next().unwrap_or(raw);
                        let label = std::str::from_utf8(until_nul)
                            .map_err(|e| format!("Invalid UTF-8 in constant string: {e}"))?;
                        if !label.is_empty() {
                            return Ok(label.to_string());
                        }
                    }
                }

                let mut len: usize = 0;
                let name_ptr = unsafe { LLVMGetValueName2(value_ref, &raw mut len) };
                if !name_ptr.is_null() && len > 0 {
                    let bytes = unsafe { std::slice::from_raw_parts(name_ptr.cast::<u8>(), len) };
                    let name = std::str::from_utf8(bytes)
                        .map_err(|e| format!("Invalid UTF-8 in LLVM value name: {e}"))?;
                    if !name.is_empty() && !name.starts_with('%') {
                        return Ok(name.to_string());
                    }
                }

                let num_operands = unsafe { LLVMGetNumOperands(value_ref) };
                for i in 0..num_operands {
                    let op = unsafe { LLVMGetOperand(value_ref, i.cast_unsigned()) };
                    if !op.is_null() {
                        stack.push(op);
                    }
                }
            }

            Err("Pointer does not reference a named global value".to_string())
        }
        BasicValueEnum::ArrayValue(_)
        | BasicValueEnum::IntValue(_)
        | BasicValueEnum::FloatValue(_)
        | BasicValueEnum::StructValue(_)
        | BasicValueEnum::VectorValue(_)
        | BasicValueEnum::ScalableVectorValue(_) => Err("GEP is not a pointer value".to_string()),
    }
}

/// Processes IR-defined functions that take qubits, replacing calls to QIR gates
/// with native calls.
///
/// # Errors
/// Returns an error if processing fails.
pub fn process_ir_defined_q_fns<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    entry_fn: FunctionValue,
) -> Result<(), String> {
    for defined_fn in module
        .get_functions()
        .filter(|f| *f != entry_fn && f.count_basic_blocks() > 0)
    {
        for bb in defined_fn.get_basic_blocks() {
            for instr in bb.get_instructions() {
                if let Ok(call) = CallSiteValue::try_from(instr) {
                    let fn_name = call
                        .get_called_fn_value()
                        .and_then(|f| f.get_name().to_str().ok().map(ToOwned::to_owned))
                        .ok_or_else(|| "Function call must have a name".to_string())?;

                    native_qir_to_qis_call(ctx, module, instr, &fn_name, defined_fn)?;
                }
            }
        }
    }
    Ok(())
}

/// Removes unreferenced IR-defined QIS helper functions left after decomposition lowering.
///
/// These functions are implementation details (e.g. `__quantum__qis__h__body`) and should not
/// remain in emitted QIS bitcode once all call sites are lowered.
pub fn prune_unused_ir_qis_helpers(module: &Module<'_>) {
    let mut called_names = std::collections::HashSet::<String>::new();
    for fun in module.get_functions() {
        for bb in fun.get_basic_blocks() {
            for instr in bb.get_instructions() {
                if let Ok(call) = CallSiteValue::try_from(instr)
                    && let Some(callee) = call.get_called_fn_value()
                    && let Ok(name) = callee.get_name().to_str()
                {
                    called_names.insert(name.to_string());
                }
            }
        }
    }

    let to_remove: Vec<_> = module
        .get_functions()
        .filter(|f| {
            f.count_basic_blocks() > 0
                && f.get_name().to_str().is_ok_and(|name| {
                    name.starts_with("__quantum__qis__") && !called_names.contains(name)
                })
        })
        .collect();

    for f in to_remove {
        // Safe here because we only delete fully-defined helper functions that have no
        // remaining call sites in the module.
        unsafe { f.delete() };
    }
}

/// Replaces calls to native QIR functions with equivalent QIS calls.
fn native_qir_to_qis_call<'a>(
    ctx: &'a Context,
    module: &Module<'a>,
    instr: InstructionValue<'a>,
    fn_name: &str,
    defined_fn: FunctionValue,
) -> Result<(), String> {
    match fn_name {
        "__quantum__qis__rxy__body" => replace_rxy_call(ctx, module, instr)?,
        "__quantum__qis__rzz__body" => replace_rzz_call(ctx, module, instr)?,
        "__quantum__qis__rz__body" => replace_rz_call(ctx, module, instr)?,
        "___qalloc" | "___reset" | "panic" => {
            if defined_fn.get_name().to_str().ok() != Some(INIT_QARRAY_FN) {
                log::error!(
                    "Unexpected call to internal function: {fn_name} in function {}",
                    defined_fn.get_name().to_str().unwrap_or("unknown")
                );
                return Err(format!("Unexpected call to internal function: {fn_name}"));
            }
        }
        _ => {
            if module
                .get_function(fn_name)
                .is_some_and(|f| f.count_basic_blocks() > 0)
            {
                // Keep IR-defined QIS helpers (e.g. decomposition functions)
                // and process their bodies in subsequent iterations.
                return Ok(());
            }
            let defined_fn_name = defined_fn.get_name().to_str().unwrap_or("unknown");
            log::error!("Unsupported function call: {fn_name} in function {defined_fn_name}");
            return Err(format!(
                "Unsupported function call: {fn_name} in {defined_fn_name}"
            ));
        }
    }
    Ok(())
}

/// Handles the output of a tuple or array by creating a new global variable
/// and recording the output.
///
/// # Errors
/// Returns an error if the output cannot be handled.
pub fn handle_tuple_or_array_output<'a, S: BuildHasher>(
    ctx: &'a Context,
    module: &Module<'a>,
    instr: InstructionValue,
    global_mapping: &mut HashMap<String, GlobalValue<'a>, S>,
    fn_name: &str,
) -> Result<(), String> {
    let args: Vec<BasicValueEnum> = (0..instr.get_num_operands())
        .map(|i| {
            let op = instr
                .get_operand(i)
                .ok_or_else(|| format!("Operand {i} not found"))?;
            match op {
                inkwell::values::Operand::Value(bv) => Ok(bv),
                inkwell::values::Operand::Block(_) => {
                    Err(format!("Operand {i} is not a BasicValueEnum"))
                }
            }
        })
        .collect::<Result<_, _>>()?;
    let length = args[0].into_int_value().as_basic_value_enum();
    let old_name = parse_gep(args[1])?;

    let full_tag = if let Some(global) = global_mapping.get(old_name.as_str()) {
        get_string_label(*global)?
    } else {
        return Err(format!("Output global `{old_name}` not found in mapping"));
    };
    // Parse the label from the global string (format: USER:RESULT:tag)
    let old_label = full_tag
        .rfind(':')
        .and_then(|pos| pos.checked_add(1))
        .map_or_else(|| full_tag.clone(), |pos| full_tag[pos..].to_string());

    let (new_const, new_name) = build_result_global(
        ctx,
        &old_label,
        &old_name,
        if fn_name == "__quantum__rt__array_record_output" {
            "QIRARRAY"
        } else {
            "QIRTUPLE"
        },
        None,
    )?;

    let new_global = module.add_global(new_const.get_type(), None, &new_name);
    new_global.set_initializer(&new_const);
    new_global.set_linkage(inkwell::module::Linkage::Private);
    new_global.set_constant(true);
    global_mapping.insert(old_name, new_global);

    let param_types = &[
        ctx.ptr_type(AddressSpace::default()).into(),
        ctx.i64_type().into(),
        ctx.i64_type().into(),
    ];
    let fn_type = ctx.void_type().fn_type(param_types, false);
    let print_func = get_or_create_function(module, "print_int", fn_type);
    record_classical_output(ctx, instr, new_global, print_func, length)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    #![allow(clippy::unwrap_used)]

    use inkwell::{types::AnyType, values::BasicValue};
    use rstest::rstest;

    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::{qir_ll_to_bc, qir_qis};

    #[test]
    fn test_is_i8_array_type_true() {
        let context = Context::create();
        let arr_ty = context.i8_type().array_type(4);
        assert!(is_i8_array_type(arr_ty.as_any_type_enum()));
    }

    #[test]
    fn test_is_i8_array_type_false() {
        let context = Context::create();
        let arr_ty = context.i32_type().array_type(4);
        assert!(!is_i8_array_type(arr_ty.as_any_type_enum()));
    }

    #[test]
    fn test_get_index_from_inttoptr() {
        // Can't test this with LLVM IR directly, but we can test the string parsing logic
        // Example IR string with inttoptr
        let ir_string = "inttoptr (i64 42 to %Qubit*)";
        let idx = get_idx_from_pointer_repr(ir_string);
        assert_eq!(idx, Ok(42));
    }

    #[test]
    fn test_get_index_from_inttoptr_opaque_ptr() {
        let ir_string = "inttoptr (i64 7 to ptr)";
        let idx = get_idx_from_pointer_repr(ir_string);
        assert_eq!(idx, Ok(7));
    }

    #[test]
    fn test_get_index_null_pointer() {
        let context = Context::create();
        let null_ptr = context
            .ptr_type(inkwell::AddressSpace::from(0))
            .const_null();
        assert_eq!(get_index(null_ptr), Ok(0));
    }

    #[test]
    fn test_get_or_create_function_adds_and_gets() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.i64_type().fn_type(&[], false);
        let func = get_or_create_function(&module, "foo", fn_type);
        assert_eq!(func.get_name().to_str().unwrap(), "foo");
        // Should return the same function if called again
        let func2 = get_or_create_function(&module, "foo", fn_type);
        assert_eq!(
            func.as_global_value().as_pointer_value(),
            func2.as_global_value().as_pointer_value()
        );
    }

    #[test]
    fn test_convert_globals_empty() {
        let context = Context::create();
        let module = context.create_module("test");
        let globals = convert_globals(&context, &module).unwrap();
        assert!(globals.is_empty());
    }

    #[test]
    fn test_get_result_vars_returns_none_vec() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("entry", fn_type, None);
        let attr = context.create_string_attribute("required_num_results", "3");
        func.add_attribute(AttributeLoc::Function, attr);
        let vars = get_result_vars(func).unwrap();
        assert_eq!(vars.len(), 3);
        assert!(vars.iter().all(std::option::Option::is_none));
    }

    #[test]
    fn test_get_result_vars_errors_without_attr() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("entry", fn_type, None);
        let result = get_result_vars(func);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Missing required_num_results");
    }

    #[test]
    fn test_find_entry_function_finds_function() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("entry", fn_type, None);
        let entry_point_attr = context.create_string_attribute("entry_point", "");
        func.add_attribute(AttributeLoc::Function, entry_point_attr);
        let found = find_entry_function(&module).unwrap();
        assert_eq!(found.get_name().to_str().unwrap(), "entry");
    }

    #[test]
    fn test_find_entry_function_errors_if_missing() {
        let context = Context::create();
        let module = context.create_module("test");
        find_entry_function(&module).unwrap_err();
    }

    #[test]
    fn test_create_qfree_call_adds_function() {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let i64_type = context.i64_type();
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("dummy", fn_type, None);
        let block = context.append_basic_block(func, "entry");
        builder.position_at_end(block);

        let handle = i64_type.const_int(42, false).as_basic_value_enum();
        create_qfree_call(&context, &module, &builder, handle);

        let qfree = module.get_function("___qfree");
        assert!(qfree.is_some());
    }

    #[test]
    fn test_create_reset_call_adds_function() {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let i64_type = context.i64_type();
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("dummy", fn_type, None);
        let block = context.append_basic_block(func, "entry");
        builder.position_at_end(block);

        let handle = i64_type.const_int(42, false).as_basic_value_enum();
        create_reset_call(&context, &module, &builder, handle);

        let reset = module.get_function("___reset");
        assert!(reset.is_some());
    }

    #[test]
    fn test_get_or_create_function_external_linkage() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.i64_type().fn_type(&[], false);
        let func = get_or_create_function(&module, "bar", fn_type);
        assert_eq!(func.get_name().to_str().unwrap(), "bar");
        assert_eq!(func.get_linkage(), Linkage::External);
    }

    #[test]
    fn test_translate_global_errors_on_long_string() {
        let context = Context::create();
        let module = context.create_module("test");
        let arr_ty = context.i8_type().array_type(256);
        let mut bytes = vec![b'a'; 255];
        bytes.push(0);
        let init = context.const_string(&bytes, false);
        let global = module.add_global(arr_ty, None, "longstr");
        global.set_initializer(&init);
        global.set_linkage(Linkage::Internal);
        global.set_constant(true);

        let mut mapping = HashMap::new();
        let mut empty_tag_counter = 0;
        assert!(
            translate_global(
                &context,
                &module,
                global,
                &mut mapping,
                &mut empty_tag_counter
            )
            .is_err()
        );
    }

    #[test]
    fn test_add_qmain_wrapper_adds_function() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.void_type().fn_type(&[], false);
        let entry_fn = module.add_function("entry", fn_type, None);
        add_qmain_wrapper(&context, &module, entry_fn).unwrap();

        let qmain = module.get_function("qmain");
        assert!(qmain.is_some());
        let setup = module.get_function("setup");
        let teardown = module.get_function("teardown");
        assert!(setup.is_some());
        assert!(teardown.is_some());
    }

    #[test]
    fn test_replace_rz_call_replaces_call() {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();
        let f64_type = context.f64_type();
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("dummy", fn_type, None);
        let block = context.append_basic_block(func, "entry");

        // Add required_num_qubits attribute to function
        let attr = context.create_string_attribute("required_num_qubits", "1");
        func.add_attribute(AttributeLoc::Function, attr);

        builder.position_at_end(block);

        let angle = f64_type.const_float(1.23).as_basic_value_enum();
        let qubit_ptr = context
            .ptr_type(inkwell::AddressSpace::from(0))
            .const_null();
        let rz_fn_type = context
            .void_type()
            .fn_type(&[f64_type.into(), qubit_ptr.get_type().into()], false);
        let rz_fn = module.add_function("__quantum__qis__rz__body", rz_fn_type, None);
        let call = builder
            .build_call(
                rz_fn,
                &[angle.into(), qubit_ptr.as_basic_value_enum().into()],
                "rzcall",
            )
            .unwrap();

        create_qubit_array(&context, &module, func).unwrap();

        let instr = call.try_as_basic_value().unwrap_instruction();
        replace_rz_call(&context, &module, instr).unwrap();

        let rz = module.get_function("___rz");
        assert!(rz.is_some());
    }

    #[test]
    fn test_translate_global_inserts_correct_mapping() {
        let context = Context::create();
        let module = context.create_module("test");
        let arr_ty = context.i8_type().array_type(6);
        let init = context.const_string(b"hello\0", false);
        let global = module.add_global(arr_ty, None, "greet");
        global.set_initializer(&init);
        global.set_linkage(Linkage::Internal);
        global.set_constant(true);

        let mut mapping = HashMap::new();
        let mut empty_tag_counter = 0;
        translate_global(
            &context,
            &module,
            global,
            &mut mapping,
            &mut empty_tag_counter,
        )
        .unwrap();

        assert!(mapping.contains_key("greet"));
        let new_global = mapping.get("greet").unwrap();
        assert_eq!(
            new_global
                .get_name()
                .to_str()
                .expect("new global name should be utf8"),
            "res_hello"
        );
    }

    #[test]
    fn test_create_qubit_array() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.void_type().fn_type(&[], false);
        let entry_fn = module.add_function("entry", fn_type, None);
        let attr = context.create_string_attribute("required_num_qubits", "2");
        entry_fn.add_attribute(AttributeLoc::Function, attr);
        let _ = context.append_basic_block(entry_fn, "entry");
        let initialize_fn_type = context.void_type().fn_type(&[], false);
        let initialize_fn =
            module.add_function("__quantum__rt__initialize", initialize_fn_type, None);
        let builder = context.create_builder();
        let entry_block = entry_fn.get_first_basic_block().unwrap();
        builder.position_at_end(entry_block);
        let _ = builder.build_call(initialize_fn, &[], "");
        let _ = create_qubit_array(&context, &module, entry_fn);
        // If it doesn't panic, it's fine
    }

    #[test]
    fn test_free_all_qubits() {
        let context = Context::create();
        let module = context.create_module("test");
        let fn_type = context.void_type().fn_type(&[], false);
        let entry_fn = module.add_function("entry", fn_type, None);
        let attr = context.create_string_attribute("required_num_qubits", "2");
        entry_fn.add_attribute(AttributeLoc::Function, attr);
        let _ = context.append_basic_block(entry_fn, "entry");
        let initialize_fn_type = context.void_type().fn_type(&[], false);
        let initialize_fn =
            module.add_function("__quantum__rt__initialize", initialize_fn_type, None);
        let builder = context.create_builder();
        let entry_block = entry_fn.get_first_basic_block().unwrap();
        builder.position_at_end(entry_block);
        let _ = builder.build_call(initialize_fn, &[], "");
        let qubit_array = create_qubit_array(&context, &module, entry_fn).unwrap();
        free_all_qubits(&context, &module, entry_fn, qubit_array).unwrap();
        // If it doesn't panic, it's fine
    }

    #[test]
    fn test_build_result_global() {
        let context = Context::create();
        let (new_const, new_name) =
            build_result_global(&context, "my_label", "old_name", "TEST", None).unwrap();

        // Check new name format
        assert_eq!(new_name, "res_my_label");

        // Check encoded length (one-byte prefix + payload)
        let expected_len = "USER:TEST:my_label".len();
        assert_eq!(new_const.get_type().len() as usize, expected_len + 1);
    }

    #[test]
    fn test_build_result_global_empty_tag_name() {
        let context = Context::create();
        let (_, new_name) = build_result_global(&context, "", "old_name", "TEST", Some(2)).unwrap();
        assert_eq!(new_name, "res_empty_tag.2");
    }

    #[test]
    fn test_handle_tuple_or_array_output_array() {
        fn test_array_output(length: u64, expected_tag: &str, name: &str) {
            let context = Context::create();
            let module = context.create_module("test");
            let builder = context.create_builder();

            // Create original global
            let arr_ty = context.i8_type().array_type(15);
            let init = context.const_string(format!("USER:RESULT:{name}\0").as_bytes(), false);
            let global = module.add_global(arr_ty, None, &format!("{name}_array"));
            global.set_initializer(&init);
            global.set_linkage(Linkage::Internal);
            global.set_constant(true);

            let mut global_mapping = HashMap::new();
            global_mapping.insert(format!("{name}_array"), global);

            // Create function and block
            let fn_type = context.void_type().fn_type(&[], false);
            let func = module.add_function("test_func", fn_type, None);
            let block = context.append_basic_block(func, "entry");
            builder.position_at_end(block);

            // Create array_record_output call instruction
            let length_val = context.i64_type().const_int(length, false);
            let gep = unsafe {
                builder
                    .build_gep(
                        arr_ty,
                        global.as_pointer_value(),
                        &[
                            context.i64_type().const_zero(),
                            context.i64_type().const_zero(),
                        ],
                        "gep",
                    )
                    .unwrap()
            };

            let record_fn_type = context
                .void_type()
                .fn_type(&[context.i64_type().into(), gep.get_type().into()], false);
            let record_fn =
                module.add_function("__quantum__rt__array_record_output", record_fn_type, None);
            let call = builder
                .build_call(record_fn, &[length_val.into(), gep.into()], "record_call")
                .unwrap();

            let instr = call.try_as_basic_value().unwrap_instruction();
            // Handle the output
            handle_tuple_or_array_output(
                &context,
                &module,
                instr,
                &mut global_mapping,
                "__quantum__rt__array_record_output",
            )
            .unwrap();

            // Check that output global mapping was updated
            let new_global = global_mapping.get(&format!("{name}_array")).unwrap();
            let new_name = new_global
                .get_name()
                .to_str()
                .expect("updated global name should be utf8");
            assert!(new_name.starts_with("res_"));
            assert_eq!(expected_tag, "QIRARRAY");

            // Verify print_int function was created for the array output
            let print_fn = module.get_function("print_int");
            assert!(print_fn.is_some());
        }

        test_array_output(0, "QIRARRAY", "empty");
        test_array_output(100, "QIRARRAY", "multi");
    }

    #[test]
    fn test_handle_tuple_or_array_output_tuple() {
        let context = Context::create();
        let module = context.create_module("test");
        let builder = context.create_builder();

        // Create original global for tuple
        let arr_ty = context.i8_type().array_type(15);
        let init = context.const_string(b"USER:RESULT:tuple\0", false);
        let global = module.add_global(arr_ty, None, "tuple_out");
        global.set_initializer(&init);
        global.set_linkage(Linkage::Internal);
        global.set_constant(true);

        let mut global_mapping = HashMap::new();
        global_mapping.insert("tuple_out".to_string(), global);

        // Create function and block
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("test_func", fn_type, None);
        let block = context.append_basic_block(func, "entry");
        builder.position_at_end(block);

        // Create tuple_record_output call instruction
        let length = context.i64_type().const_int(2, false); // Tuple size
        let gep = unsafe {
            builder
                .build_gep(
                    arr_ty,
                    global.as_pointer_value(),
                    &[
                        context.i64_type().const_zero(),
                        context.i64_type().const_zero(),
                    ],
                    "gep",
                )
                .unwrap()
        };

        let record_fn_type = context
            .void_type()
            .fn_type(&[context.i64_type().into(), gep.get_type().into()], false);
        let record_fn =
            module.add_function("__quantum__rt__tuple_record_output", record_fn_type, None);
        let call = builder
            .build_call(record_fn, &[length.into(), gep.into()], "record_call")
            .unwrap();

        let instr = call.try_as_basic_value().unwrap_instruction();
        // Handle the output
        handle_tuple_or_array_output(
            &context,
            &module,
            instr,
            &mut global_mapping,
            "__quantum__rt__tuple_record_output",
        )
        .unwrap();

        // Check that output global mapping was updated
        let new_global = global_mapping.get("tuple_out").unwrap();
        let new_name = new_global
            .get_name()
            .to_str()
            .expect("updated global name should be utf8");
        assert!(new_name.starts_with("res_"));
    }

    #[test]
    fn test_parse_gep() {
        let context = Context::create();
        let builder = context.create_builder();
        let module = context.create_module("test");

        // Create a global variable
        let arr_ty = context.i8_type().array_type(10);
        let init = context.const_string(b"test\0", false);
        let global = module.add_global(arr_ty, None, "test_global");
        global.set_initializer(&init);

        // Create a function to test in
        let fn_type = context.void_type().fn_type(&[], false);
        let func = module.add_function("test_func", fn_type, None);
        let block = context.append_basic_block(func, "entry");
        builder.position_at_end(block);

        // Build GEP instruction
        let gep = unsafe {
            builder
                .build_gep(
                    arr_ty,
                    global.as_pointer_value(),
                    &[
                        context.i32_type().const_zero(),
                        context.i32_type().const_zero(),
                    ],
                    "gep",
                )
                .unwrap()
        };

        // Parse the GEP
        let global_name = parse_gep(gep.as_basic_value_enum()).unwrap();
        assert_eq!(global_name, "test_global");
    }

    fn get_qir_bytes(ll_path: &Path) -> Vec<u8> {
        let ll = fs::read_to_string(ll_path).expect("Failed to read input LLVM IR file");
        qir_ll_to_bc(&ll).expect("Failed to convert LLVM IR to bitcode")
    }

    #[test]
    fn test_ir_fn_main_errors() {
        let ll_path = Path::new("tests/data/bad/ir_fn_main.ll");
        let qir_bytes = get_qir_bytes(ll_path);

        assert!(qir_qis::qir_to_qis(qir_bytes.into(), 2, "aarch64", None).is_err());
    }

    #[test]
    fn test_unknown_fn_errors() {
        let ll_path = Path::new("tests/data/bad/mz_to_creg_bit.ll");
        let qir_bytes = get_qir_bytes(ll_path);

        assert!(qir_qis::validate_qir(qir_bytes.clone().into(), None).is_err());
        assert!(qir_qis::qir_to_qis(qir_bytes.into(), 2, "aarch64", None).is_err());
    }

    #[test]
    fn test_missing_label_errors() {
        let ll_path = Path::new("tests/data/bad/pytket_qir_12.ll");
        let qir_bytes = get_qir_bytes(ll_path);

        assert!(qir_qis::qir_to_qis(qir_bytes.into(), 2, "aarch64", None).is_err());
    }

    #[test]
    fn test_barrier_invalid_fails_validation() {
        let ll_path = Path::new("tests/data/bad/barrier_invalid.ll");
        let qir_bytes = get_qir_bytes(ll_path);

        assert!(qir_qis::validate_qir(qir_bytes.into(), None).is_err());
    }

    #[test]
    fn test_get_string_label() {
        let context = Context::create();
        let module = context.create_module("test");

        let arr_ty = context.i8_type().array_type(20);
        let init = context.const_string(b"USER:RESULT:my_label\0", false);
        let global = module.add_global(arr_ty, None, "my_global");
        global.set_initializer(&init);

        let label = get_string_label(global).unwrap();
        assert_eq!(label, "USER:RESULT:my_label");
    }

    #[test]
    fn test_get_string_label_empty() {
        let context = Context::create();
        let module = context.create_module("test");

        let arr_ty = context.i8_type().array_type(1);
        let init = context.const_string(b"\0", false);
        let global = module.add_global(arr_ty, None, "empty_global");
        global.set_initializer(&init);

        let label = get_string_label(global).unwrap();
        assert_eq!(label, "");
    }

    macro_rules! snapshot_cases {
        ($item:item) => {
            #[rstest]
            // Base profile tests
            #[case("tests/data/base_native_only.ll")]
            #[case("tests/data/base.ll")]
            #[case("tests/data/base_array.ll")]
            #[case("tests/data/barrier.ll")]
            #[case("tests/data/barrier_multi.ll")]
            // Adaptive profile tests
            #[case("tests/data/adaptive.ll")]
            #[case("tests/data/adaptive_ir_fns.ll")]
            #[case("tests/data/adaptive_iter.ll")]
            #[case("tests/data/adaptive_iter_fn.ll")]
            #[case("tests/data/adaptive_cond_loop.ll")]
            #[case("tests/data/adaptive_multi_ret.ll")]
            // QIR 2.0 (opaque pointers) tests
            #[case("tests/data/qir2_base.ll")]
            #[case("tests/data/qir2_adaptive.ll")]
            // Infinite loop test
            #[case("tests/data/bad/inf_loop.ll")]
            // RNG and get shot number test with Adaptive-profile switch
            #[case("tests/data/ArithOps_switch.ll")]
            #[trace]
            $item
        };
    }

    #[cfg(not(windows))]
    snapshot_cases! {
    fn test_snapshot_conversion(#[case] llpath: &str) {
        use insta::Settings;

        let ll_path = Path::new(llpath);
        let qir_bytes = get_qir_bytes(ll_path);
        let qis_bytes = qir_qis::qir_to_qis(qir_bytes.into(), 2, "aarch64", None).unwrap();

        let context = Context::create();
        let memory_buffer = inkwell::memory_buffer::MemoryBuffer::create_from_memory_range_copy(
            &qis_bytes,
            "qis_module",
        );
        let qis_text = Module::parse_bitcode_from_buffer(&memory_buffer, &context)
            .expect("Compiled QIS bitcode should parse");

        let mut settings = Settings::clone_current();
        settings.set_prepend_module_to_snapshot(false);
        settings.set_snapshot_path("../tests/snaps");

        let filename = llpath.split('/').skip(2).collect::<Vec<_>>().join("/");

        settings.bind(|| {
            insta::with_settings!({filters => vec![
                (r#"@gen_version = local_unnamed_addr global \[[0-9]+ x i8\] c"[^"]+", section ",generator""#,
                  r#"@gen_version = local_unnamed_addr global [5 x i8] c"0.0.0", section ",generator""#),
            ]}, {
                insta::assert_snapshot!(filename, qis_text.to_string());
            });
        });
    }}

    #[cfg(windows)]
    snapshot_cases! {
    // Windows runs this as a smoke test instead of snapshot matching because
    // cross-target (`aarch64`) optimized codegen in CI has shown backend
    // instability and non-deterministic output differences.
    fn test_snapshot_conversion_windows_smoke(#[case] llpath: &str) {
        let ll_path = Path::new(llpath);
        let qir_bytes = get_qir_bytes(ll_path);
        // Keep this as a pure conversion/parsing smoke test on Windows.
        // TargetMachine creation for optimized native codegen can be unstable
        // on some Windows LLVM environments and cause access violations.
        let qis_bytes = qir_qis::qir_to_qis(qir_bytes.into(), 0, "native", None).unwrap();

        let context = Context::create();
        let memory_buffer = inkwell::memory_buffer::MemoryBuffer::create_from_memory_range_copy(
            &qis_bytes,
            "qis_module",
        );
        let parsed = Module::parse_bitcode_from_buffer(&memory_buffer, &context)
            .expect("Compiled QIS bitcode should parse on Windows");
        assert!(parsed.get_function("qmain").is_some());
    }}
}
