//! Result storage shared by scalar and array lowering.
//!
//! Static IDs index a private table; dynamic IDs own heap slots. Keeping slot
//! cleanup separate from deallocation lets both representations share futures.
#![allow(
    clippy::expect_used,
    reason = "builder operations construct fixed, internally checked LLVM helper layouts"
)]

use std::collections::{HashMap, HashSet};

use inkwell::{
    AddressSpace, IntPredicate,
    attributes::{Attribute, AttributeLoc},
    builder::Builder,
    context::Context,
    module::{Linkage, Module},
    types::FunctionType,
    values::{
        AsValueRef, BasicValue, CallSiteValue, FunctionValue, InstructionValue, PointerValue,
    },
};

use crate::{
    aux::{build_dynamic_array_loop, build_ptr_is_null, ensure_out_err_success},
    convert::{build_panic_with_message, get_or_create_function, get_required_num_results},
};

pub const STATIC_RESULTS: &str = "qir_qis.static_results";

/// The QIR result contract is shared by admission and lowering selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultOp {
    Allocate,
    Release,
    ArrayAllocate,
    ArrayRelease,
    ArrayOutput,
    Read,
    Output,
    Measure,
    MeasureAlias,
    MeasureReset,
}

impl ResultOp {
    const ALL: [(&str, Self); 10] = [
        ("__quantum__rt__result_allocate", Self::Allocate),
        ("__quantum__rt__result_release", Self::Release),
        ("__quantum__rt__result_array_allocate", Self::ArrayAllocate),
        ("__quantum__rt__result_array_release", Self::ArrayRelease),
        (
            "__quantum__rt__result_array_record_output",
            Self::ArrayOutput,
        ),
        ("__quantum__rt__read_result", Self::Read),
        ("__quantum__rt__result_record_output", Self::Output),
        ("__quantum__qis__mz__body", Self::Measure),
        ("__quantum__qis__m__body", Self::MeasureAlias),
        ("__quantum__qis__mresetz__body", Self::MeasureReset),
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find_map(|(candidate, op)| (*candidate == name).then_some(*op))
    }

    fn signature(self, ctx: &Context) -> FunctionType<'_> {
        let ptr = ctx.ptr_type(AddressSpace::default());
        match self {
            Self::Allocate => ptr.fn_type(&[ptr.into()], false),
            Self::Release => ctx.void_type().fn_type(&[ptr.into()], false),
            Self::ArrayAllocate | Self::ArrayOutput => ctx
                .void_type()
                .fn_type(&[ctx.i64_type().into(), ptr.into(), ptr.into()], false),
            Self::ArrayRelease => ctx
                .void_type()
                .fn_type(&[ctx.i64_type().into(), ptr.into()], false),
            Self::Read => ctx.bool_type().fn_type(&[ptr.into()], false),
            Self::Output | Self::Measure | Self::MeasureAlias | Self::MeasureReset => {
                ctx.void_type().fn_type(&[ptr.into(), ptr.into()], false)
            }
        }
    }

    pub fn capability_error(
        self,
        name: &str,
        flags: crate::aux::CapabilityFlags,
    ) -> Option<String> {
        let requirement = match self {
            Self::ArrayOutput if !flags.arrays => "`arrays=true`",
            Self::ArrayAllocate | Self::ArrayRelease
                if !flags.arrays || !flags.dynamic_result_management =>
            {
                "both `arrays=true` and `dynamic_result_management=true`"
            }
            Self::Allocate | Self::Release if !flags.dynamic_result_management => {
                "`dynamic_result_management=true`"
            }
            Self::Allocate
            | Self::Release
            | Self::ArrayAllocate
            | Self::ArrayRelease
            | Self::ArrayOutput
            | Self::Read
            | Self::Output
            | Self::Measure
            | Self::MeasureAlias
            | Self::MeasureReset => return None,
        };
        Some(format!("{name} requires {requirement}"))
    }

    const fn result_operand(self) -> Option<u32> {
        match self {
            Self::Read | Self::Output => Some(0),
            Self::Measure | Self::MeasureAlias | Self::MeasureReset => Some(1),
            Self::Allocate
            | Self::Release
            | Self::ArrayAllocate
            | Self::ArrayRelease
            | Self::ArrayOutput => None,
        }
    }

    const fn tag_operand(self) -> Option<u32> {
        match self {
            Self::Output => Some(1),
            Self::ArrayOutput => Some(2),
            Self::Allocate
            | Self::Release
            | Self::ArrayAllocate
            | Self::ArrayRelease
            | Self::Read
            | Self::Measure
            | Self::MeasureAlias
            | Self::MeasureReset => None,
        }
    }

    pub const fn is_array(self) -> bool {
        matches!(
            self,
            Self::ArrayAllocate | Self::ArrayRelease | Self::ArrayOutput
        )
    }
}

/// Unlike Inkwell's type-filtered accessor, this exposes mismatched direct calls
/// so admission can reject them before any operand conversions occur.
fn direct_callee(call: CallSiteValue<'_>) -> Option<FunctionValue<'_>> {
    // SAFETY: `call` is a live call instruction; FunctionValue::new checks its kind.
    unsafe { FunctionValue::new(llvm_sys::core::LLVMGetCalledValue(call.as_value_ref())) }
}

pub fn snapshot_calls<'ctx>(
    module: &Module<'ctx>,
) -> Vec<(FunctionValue<'ctx>, InstructionValue<'ctx>)> {
    let mut calls = Vec::new();
    for function in module.get_functions() {
        for block in function.get_basic_blocks() {
            for instruction in block.get_instructions() {
                if CallSiteValue::try_from(instruction).is_ok() {
                    calls.push((function, instruction));
                }
            }
        }
    }
    calls
}

fn remove_effect_attributes(count: u32, mut remove: impl FnMut(AttributeLoc, u32)) {
    for name in [
        "memory",
        "readnone",
        "readonly",
        "writeonly",
        "argmemonly",
        "inaccessiblememonly",
        "inaccessiblemem_or_argmemonly",
        "nofree",
        "nosync",
        "speculatable",
        "willreturn",
    ] {
        let kind = Attribute::get_named_enum_kind_id(name);
        if kind != 0 {
            remove(AttributeLoc::Function, kind);
        }
    }
    for index in 0..count {
        for name in ["readonly", "writeonly", "readnone"] {
            let kind = Attribute::get_named_enum_kind_id(name);
            if kind != 0 {
                remove(AttributeLoc::Param(index), kind);
            }
        }
    }
}

/// Result reads cache state and measurements release old futures. Their callers
/// must not retain the input QIR's narrower memory or progress promises.
pub fn clear_result_effect_attributes(
    ctx: &Context,
    module: &Module<'_>,
    calls: &[(FunctionValue<'_>, InstructionValue<'_>)],
) {
    let mut callers = HashMap::<_, Vec<_>>::new();
    let mut pending = Vec::new();
    for &(function, instruction) in calls {
        if let Ok(call) = CallSiteValue::try_from(instruction)
            && let Some(callee) = direct_callee(call)
        {
            callers.entry(callee).or_default().push(function);
            if callee
                .get_name()
                .to_str()
                .ok()
                .and_then(ResultOp::from_name)
                .is_some()
            {
                pending.push(function);
            }
        }
    }
    // Reused downstream declarations also acquire calls from the slot helpers.
    for (name, _) in downstream_symbols(ctx) {
        if let Some(function) = module.get_function(name) {
            pending.push(function);
        }
    }
    let mut affected = HashSet::new();
    while let Some(function) = pending.pop() {
        if affected.insert(function)
            && let Some(parents) = callers.get(&function)
        {
            pending.extend(parents);
        }
    }
    for function in &affected {
        remove_effect_attributes(function.count_params(), |loc, kind| {
            function.remove_enum_attribute(loc, kind);
        });
    }
    for &(_, instruction) in calls {
        if let Ok(call) = CallSiteValue::try_from(instruction)
            && direct_callee(call).is_some_and(|callee| affected.contains(&callee))
        {
            remove_effect_attributes(call.count_arguments(), |loc, kind| {
                call.remove_enum_attribute(loc, kind);
            });
        }
    }
}

pub fn validate_input_symbols(ctx: &Context, module: &Module<'_>) -> Result<(), String> {
    validate_alias_symbols(ctx, module)?;
    validate_heap_runtime_symbols(module)?;
    for global in module.get_globals() {
        if global
            .get_name()
            .to_str()
            .ok()
            .and_then(ResultOp::from_name)
            .is_some()
        {
            return Err(format!(
                "QIR result function must be an external declaration: {}",
                global.get_name().to_string_lossy()
            ));
        }
        if global.get_name().to_bytes().starts_with(b"qir_qis.") {
            return Err("Input QIR must not define internal result storage globals".into());
        }
    }
    for function in module.get_functions() {
        if let Some(op) = function
            .get_name()
            .to_str()
            .ok()
            .and_then(ResultOp::from_name)
        {
            let name = function.get_name().to_string_lossy();
            if function.count_basic_blocks() != 0 {
                return Err(format!(
                    "QIR result function must be an external declaration: {name}"
                ));
            }
            if function.get_type() != op.signature(ctx) {
                let kind = if op.result_operand().is_some() {
                    "result"
                } else {
                    "RT"
                };
                return Err(format!("Malformed QIR {kind} function declaration: {name}"));
            }
        }
        if function.get_name().to_bytes().starts_with(b"qir_qis.") {
            return Err(format!(
                "Input QIR must not define internal helper function: {}",
                function.get_name().to_string_lossy()
            ));
        }
    }
    validate_downstream_result_symbols(ctx, module)
}

fn downstream_symbols(ctx: &Context) -> [(&str, FunctionType<'_>); 9] {
    let ptr = ctx.ptr_type(AddressSpace::default());
    [
        ("heap_alloc", ptr.fn_type(&[ctx.i64_type().into()], false)),
        ("heap_free", ctx.void_type().fn_type(&[ptr.into()], false)),
        (
            "___reset",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        ),
        (
            "panic",
            ctx.void_type()
                .fn_type(&[ctx.i32_type().into(), ptr.into()], false),
        ),
        (
            "___read_future_bool",
            ctx.bool_type().fn_type(&[ctx.i64_type().into()], false),
        ),
        (
            "___dec_future_refcount",
            ctx.void_type().fn_type(&[ctx.i64_type().into()], false),
        ),
        (
            "___lazy_measure",
            ctx.i64_type().fn_type(&[ctx.i64_type().into()], false),
        ),
        (
            "print_bool",
            ctx.void_type().fn_type(
                &[ptr.into(), ctx.i64_type().into(), ctx.bool_type().into()],
                false,
            ),
        ),
        (
            "print_bool_arr",
            ctx.void_type()
                .fn_type(&[ptr.into(), ctx.i64_type().into(), ptr.into()], false),
        ),
    ]
}

fn validate_downstream_result_symbols(ctx: &Context, module: &Module<'_>) -> Result<(), String> {
    for (name, ty) in downstream_symbols(ctx) {
        if module.get_global(name).is_some()
            || module
                .get_function(name)
                .is_some_and(|f| f.get_type() != ty || f.count_basic_blocks() != 0)
        {
            return Err(format!("Invalid downstream runtime declaration: {name}"));
        }
    }
    Ok(())
}

fn validate_heap_runtime_symbols(module: &Module<'_>) -> Result<(), String> {
    // Only lowering may introduce heap calls; downstream attributes must not
    // grant input QIR access to the allocator used for result storage.
    for name in ["heap_alloc", "heap_free"] {
        if module.get_function(name).is_some() || module.get_global(name).is_some() {
            return Err(format!(
                "Input QIR uses reserved heap runtime symbol: {name}"
            ));
        }
    }
    Ok(())
}

#[derive(Default)]
pub struct ResultValidation {
    pub errors: Vec<String>,
    pub capabilities: Vec<String>,
}

pub fn validate_result_calls(ctx: &Context, module: &Module<'_>) -> ResultValidation {
    let mut checks = ResultValidation::default();
    let flags = crate::aux::get_capability_flags(module);
    let Ok(entry) = crate::convert::find_entry_function(module) else {
        return checks;
    };
    // Missing metadata is already reported by entry validation (and required by
    // storage initialization during translation). Do not report it twice.
    let required = if flags.dynamic_result_management
        || entry
            .get_string_attribute(AttributeLoc::Function, "required_num_results")
            .is_none()
    {
        None
    } else {
        match get_required_num_results(entry) {
            Ok(count) => Some(count),
            Err(error) => {
                checks.errors.push(error);
                None
            }
        }
    };
    let table = uses_static_results(module, entry, flags.dynamic_result_management);
    for function in module.get_functions() {
        for block in function.get_basic_blocks() {
            for instruction in block.get_instructions() {
                let Ok(call) = CallSiteValue::try_from(instruction) else {
                    continue;
                };
                let Some(callee) = direct_callee(call) else {
                    continue;
                };
                let name = callee.get_name();
                let Some((name, op)) = name
                    .to_str()
                    .ok()
                    .and_then(|name| ResultOp::from_name(name).map(|op| (name, op)))
                else {
                    continue;
                };
                if let Some(error) = op.capability_error(name, flags) {
                    checks.capabilities.push(error);
                }
                if call.get_called_fn_type() != op.signature(ctx)
                    || call.count_arguments() != callee.count_params()
                {
                    checks
                        .errors
                        .push(format!("Malformed QIR result call: {name}"));
                    // Keep the existing backing diagnostic for malformed array
                    // operands, without interpreting a callee operand as an argument.
                    if op.is_array()
                        && let Ok(mut args) = crate::utils::extract_operands(&instruction)
                    {
                        args.truncate(call.count_arguments() as usize);
                        if let Err(error) = crate::aux::validate_pointer_array_call(name, &args) {
                            checks.errors.push(error);
                        }
                    }
                    continue;
                }
                if let Err(error) =
                    validate_result_operands(ctx, instruction, op, required, table, name)
                {
                    checks.errors.push(error);
                }
            }
        }
    }
    checks
}

fn validate_result_operands(
    ctx: &Context,
    instruction: InstructionValue<'_>,
    op: ResultOp,
    required: Option<usize>,
    table: bool,
    name: &str,
) -> Result<(), String> {
    if let (Some(count), Some(index)) = (required, op.result_operand()) {
        let result = instruction
            .get_operand(index)
            .and_then(inkwell::values::Operand::value)
            .ok_or("Missing result operand")?
            .into_pointer_value();
        match crate::convert::get_index(result) {
            Ok(index) => {
                crate::aux::checked_result_index(index, count)?;
            }
            Err(_) if count == 0 => {
                return Err("Scalar result operations require at least one static result".into());
            }
            Err(_) if table => {}
            Err(error) => {
                return Err(format!(
                    "Failed to inspect result operand for `{name}`: {error}"
                ));
            }
        }
    }
    if op.is_array() {
        let args = crate::utils::extract_operands(&instruction)?;
        crate::aux::validate_pointer_array_call(name, &args)?;
    }
    let Some(index) = op.tag_operand() else {
        return Ok(());
    };
    let tag = instruction
        .get_operand(index)
        .and_then(inkwell::values::Operand::value)
        .ok_or("Missing result output tag")?;
    match crate::convert::resolve_output_tag(tag)? {
        None if op == ResultOp::ArrayOutput => {
            return Err("Result array output requires a constant global tag".into());
        }
        Some(global) => {
            let label = crate::convert::get_string_label(global)?;
            crate::convert::build_result_global(
                ctx,
                &label,
                "",
                if op == ResultOp::ArrayOutput {
                    "RESULT_ARRAY"
                } else {
                    "RESULT"
                },
                None,
            )?;
        }
        None => {}
    }
    Ok(())
}

fn validate_alias_symbols(ctx: &Context, module: &Module<'_>) -> Result<(), String> {
    use llvm_sys::core::{
        LLVMGetFirstGlobalAlias, LLVMGetFirstGlobalIFunc, LLVMGetNextGlobalAlias,
        LLVMGetNextGlobalIFunc, LLVMGetValueName2,
    };
    // Inkwell's global iterator visits variables, not aliases or indirect functions.
    for (first, next) in [
        (
            LLVMGetFirstGlobalAlias
                as unsafe extern "C" fn(
                    llvm_sys::prelude::LLVMModuleRef,
                ) -> llvm_sys::prelude::LLVMValueRef,
            LLVMGetNextGlobalAlias
                as unsafe extern "C" fn(
                    llvm_sys::prelude::LLVMValueRef,
                ) -> llvm_sys::prelude::LLVMValueRef,
        ),
        (
            LLVMGetFirstGlobalIFunc
                as unsafe extern "C" fn(
                    llvm_sys::prelude::LLVMModuleRef,
                ) -> llvm_sys::prelude::LLVMValueRef,
            LLVMGetNextGlobalIFunc
                as unsafe extern "C" fn(
                    llvm_sys::prelude::LLVMValueRef,
                ) -> llvm_sys::prelude::LLVMValueRef,
        ),
    ] {
        let mut value = unsafe { first(module.as_mut_ptr()) };
        while !value.is_null() {
            let mut len = 0;
            // SAFETY: these iterators yield live GlobalAlias/GlobalIFunc values;
            // LLVMGetValueName2 returns their borrowed name and exact byte length.
            let pointer = unsafe { LLVMGetValueName2(value, &raw mut len) };
            if !pointer.is_null() {
                let name = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), len) };
                if name.starts_with(b"qir_qis.")
                    || downstream_symbols(ctx)
                        .iter()
                        .any(|(symbol, _)| symbol.as_bytes() == name)
                    || std::str::from_utf8(name)
                        .ok()
                        .and_then(ResultOp::from_name)
                        .is_some()
                {
                    return Err(format!(
                        "Input alias or indirect function uses reserved symbol: {}",
                        String::from_utf8_lossy(name)
                    ));
                }
            }
            value = unsafe { next(value) };
        }
    }
    Ok(())
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

#[allow(
    clippy::unreachable,
    reason = "a runtime helper returning no basic value indicates a broken lowering invariant"
)]
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

pub fn get_dynamic_result_slot_type(ctx: &Context) -> inkwell::types::StructType<'_> {
    ctx.struct_type(
        &[
            ctx.i8_type().into(),   // state: 0=false, 1=pending future, 2=cached bool
            ctx.bool_type().into(), // cached bool
            ctx.i64_type().into(),  // future handle
        ],
        false,
    )
}

pub fn initialize_dynamic_result_slot<'ctx>(
    ctx: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    slot_ptr: PointerValue<'ctx>,
) {
    let slot_type = get_dynamic_result_slot_type(ctx);
    let slot_ptrs = get_dynamic_result_slot_ptrs(builder, slot_type, slot_ptr);
    clear_dynamic_result_slot(ctx, builder, &slot_ptrs);
}

pub fn ensure_dynamic_result_setter<'ctx>(
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
    let clear = ensure_result_slot_clear(ctx, module);
    builder
        .build_call(clear, &[result_ptr.into()], "")
        .expect("clear previous measurement");
    let slot_ptrs = get_dynamic_result_slot_ptrs(&builder, slot_type, result_ptr);
    set_dynamic_result_pending(ctx, &builder, &slot_ptrs, future);
    builder.build_return(None).expect("ret");
    function
}

pub fn ensure_dynamic_result_read<'ctx>(
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
    let is_null = build_ptr_is_null(ctx, &builder, result_ptr, "result_int").expect("null check");
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

pub fn ensure_result_slot_clear<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function("qir_qis.result_clear") {
        return existing;
    }
    let ptr_type = ctx.ptr_type(AddressSpace::default());
    let slot_type = get_dynamic_result_slot_type(ctx);
    let function = module.add_function(
        "qir_qis.result_clear",
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
    let is_null = build_ptr_is_null(ctx, &builder, result_ptr, "result_int").expect("null check");
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

pub fn ensure_dynamic_result_array_release<'ctx>(
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
    build_dynamic_array_loop(ctx, function, &builder, len, |builder, index| {
        // SAFETY: the caller owns a validated pointer array of `len` entries.
        let element = unsafe { builder.build_gep(ptr_type, array_ptr, &[index], "elem_ptr") }
            .map_err(|error| error.to_string())?;
        let value = builder
            .build_load(ptr_type, element, "value_ptr")
            .map_err(|error| error.to_string())?;
        builder
            .build_call(release_fn, &[value.into()], "")
            .map_err(|error| error.to_string())?;
        Ok(())
    })
    .expect("build dynamic result release loop");
    builder.build_return(None).expect("ret");
    function
}

pub fn ensure_dynamic_result_array_record_output<'ctx>(
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
    // Keep variable-size output scratch in its own call frame inside loops.
    function.add_attribute(
        AttributeLoc::Function,
        ctx.create_enum_attribute(
            inkwell::attributes::Attribute::get_named_enum_kind_id("noinline"),
            0,
        ),
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
    let read_fn = result_read_function(ctx, module, true).ok_or("Missing result array reader")?;
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

fn report_allocation_failure<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(f) = module.get_function("qir_qis.result_allocation_failed") {
        return f;
    }
    let ptr = ctx.ptr_type(AddressSpace::default());
    let f = module.add_function(
        "qir_qis.result_allocation_failed",
        ctx.void_type().fn_type(&[ptr.into()], false),
        Some(Linkage::Private),
    );
    let b = ctx.create_builder();
    let entry = ctx.append_basic_block(f, "entry");
    let terminate = ctx.append_basic_block(f, "terminate");
    let flag = ctx.append_basic_block(f, "flag");
    b.position_at_end(entry);
    let out_err = f.get_first_param().expect("out_err").into_pointer_value();
    let null = b
        .build_is_null(out_err, "null_out_err")
        .expect("null check");
    b.build_conditional_branch(null, terminate, flag)
        .expect("failure dispatch");
    b.position_at_end(terminate);
    build_panic_with_message(
        ctx,
        module,
        &b,
        "qir_qis.result_allocation_error",
        "No memory available for result allocation.",
    )
    .expect("allocation panic message");
    b.position_at_end(flag);
    b.build_store(out_err, ctx.bool_type().const_all_ones())
        .expect("failure flag");
    b.build_return(None).expect("return failure");
    f
}

pub fn ensure_dynamic_result_allocate<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(f) = module.get_function("qir_qis.result_allocate") {
        return f;
    }
    let ptr = ctx.ptr_type(AddressSpace::default());
    let f = module.add_function(
        "qir_qis.result_allocate",
        ptr.fn_type(&[ptr.into()], false),
        Some(Linkage::Private),
    );
    let b = ctx.create_builder();
    let entry = ctx.append_basic_block(f, "entry");
    let failure = ctx.append_basic_block(f, "failure");
    let success = ctx.append_basic_block(f, "success");
    b.position_at_end(entry);
    let out_err = f.get_first_param().expect("out_err").into_pointer_value();
    let heap = get_or_create_function(
        module,
        "heap_alloc",
        ptr.fn_type(&[ctx.i64_type().into()], false),
    );
    // LLVM computes the allocation size using the eventual target's struct layout.
    let size = get_dynamic_result_slot_type(ctx)
        .size_of()
        .expect("sized result slot");
    let slot = b
        .build_call(heap, &[size.into()], "slot")
        .expect("allocate slot")
        .try_as_basic_value()
        .basic()
        .expect("heap pointer")
        .into_pointer_value();
    let failed = b.build_is_null(slot, "failed").expect("allocation check");
    b.build_conditional_branch(failed, failure, success)
        .expect("allocation dispatch");
    b.position_at_end(failure);
    b.build_call(
        report_allocation_failure(ctx, module),
        &[out_err.into()],
        "",
    )
    .expect("report failure");
    b.build_return(Some(&ptr.const_null()))
        .expect("return null");
    b.position_at_end(success);
    initialize_dynamic_result_slot(ctx, &b, slot);
    b.build_call(ensure_out_err_success(ctx, module), &[out_err.into()], "")
        .expect("success flag");
    b.build_return(Some(&slot)).expect("return slot");
    f
}

pub fn ensure_dynamic_result_release<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(f) = module.get_function("qir_qis.result_release") {
        return f;
    }
    let ptr = ctx.ptr_type(AddressSpace::default());
    let f = module.add_function(
        "qir_qis.result_release",
        ctx.void_type().fn_type(&[ptr.into()], false),
        Some(Linkage::Private),
    );
    let b = ctx.create_builder();
    b.position_at_end(ctx.append_basic_block(f, "entry"));
    let slot = f.get_first_param().expect("slot");
    // QIR requires non-null handles to name live allocations. Repeated release
    // and use after release are caller-side undefined behavior, not tracked here.
    b.build_call(ensure_result_slot_clear(ctx, module), &[slot.into()], "")
        .expect("clear slot");
    let free = get_or_create_function(
        module,
        "heap_free",
        ctx.void_type().fn_type(&[ptr.into()], false),
    );
    // Both slot cleanup and heap_free accept null.
    b.build_call(free, &[slot.into()], "").expect("free slot");
    b.build_return(None).expect("return release");
    f
}

pub fn ensure_dynamic_result_array_allocate<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(f) = module.get_function("qir_qis.result_array_allocate") {
        return f;
    }
    let ptr = ctx.ptr_type(AddressSpace::default());
    let i64t = ctx.i64_type();
    let f = module.add_function(
        "qir_qis.result_array_allocate",
        ctx.void_type()
            .fn_type(&[i64t.into(), ptr.into(), ptr.into()], false),
        Some(Linkage::Private),
    );
    let b = ctx.create_builder();
    let entry = ctx.append_basic_block(f, "entry");
    let header = ctx.append_basic_block(f, "header");
    let body = ctx.append_basic_block(f, "body");
    let next = ctx.append_basic_block(f, "next");
    let rollback = ctx.append_basic_block(f, "rollback");
    let done = ctx.append_basic_block(f, "done");
    b.position_at_end(entry);
    let len = f.get_nth_param(0).expect("length").into_int_value();
    let array = f.get_nth_param(1).expect("array").into_pointer_value();
    let out_err = f.get_nth_param(2).expect("out_err").into_pointer_value();
    // Force recoverable per-element failures so rollback also precedes panic.
    let local_error = b
        .build_alloca(ctx.bool_type(), "allocation_error")
        .expect("local error flag");
    b.build_call(ensure_out_err_success(ctx, module), &[out_err.into()], "")
        .expect("success flag");
    b.build_unconditional_branch(header).expect("start loop");
    b.position_at_end(header);
    let index = b.build_phi(i64t, "index").expect("index phi");
    index.add_incoming(&[(&i64t.const_zero(), entry)]);
    let idx = index.as_basic_value().into_int_value();
    let more = b
        .build_int_compare(IntPredicate::ULT, idx, len, "more")
        .expect("length check");
    b.build_conditional_branch(more, body, done)
        .expect("loop branch");
    b.position_at_end(body);
    let slot = b
        .build_call(
            ensure_dynamic_result_allocate(ctx, module),
            &[local_error.into()],
            "slot",
        )
        .expect("allocate element")
        .try_as_basic_value()
        .basic()
        .expect("result pointer")
        .into_pointer_value();
    let element = unsafe { b.build_gep(ptr, array, &[idx], "element") }.expect("element pointer");
    b.build_store(element, slot).expect("store element");
    let failed = b.build_is_null(slot, "failed").expect("allocation check");
    b.build_conditional_branch(failed, rollback, next)
        .expect("element dispatch");
    b.position_at_end(next);
    let next_idx = b
        .build_int_add(idx, i64t.const_int(1, false), "next_index")
        .expect("increment");
    b.build_unconditional_branch(header).expect("next element");
    index.add_incoming(&[(&next_idx, next)]);
    b.position_at_end(rollback);
    b.build_call(
        ensure_dynamic_result_array_release(ctx, module),
        &[idx.into(), array.into()],
        "",
    )
    .expect("rollback elements");
    build_dynamic_array_loop(ctx, f, &b, idx, |b, i| {
        let element = unsafe { b.build_gep(ptr, array, &[i], "released_element") }
            .map_err(|e| e.to_string())?;
        b.build_store(element, ptr.const_null())
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .expect("clear rolled back entries");
    b.build_call(
        report_allocation_failure(ctx, module),
        &[out_err.into()],
        "",
    )
    .expect("report bulk failure");
    b.build_return(None).expect("return bulk failure");
    b.position_at_end(done);
    b.build_return(None).expect("return bulk success");
    f
}

pub fn validate_translation_input(ctx: &Context, module: &Module<'_>) -> Result<(), String> {
    validate_input_symbols(ctx, module)?;
    let checks = validate_result_calls(ctx, module);
    let mut errors = checks.errors;
    errors.extend(checks.capabilities);
    crate::aux::validate_dynamic_array_allocation_backing(module, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn uses_static_results(module: &Module<'_>, entry: FunctionValue<'_>, dynamic: bool) -> bool {
    if dynamic {
        return false;
    }
    if module
        .get_function("__quantum__rt__result_array_record_output")
        .is_some_and(|output| {
            output
                .as_global_value()
                .as_pointer_value()
                .get_first_use()
                .is_some()
        })
    {
        return true;
    }
    module
        .get_functions()
        .filter(|function| {
            *function != entry && !function.get_name().to_bytes().starts_with(b"qir_qis.")
        })
        .any(|function| {
            function.get_basic_blocks().into_iter().any(|block| {
                block.get_instructions().any(|instruction| {
                    CallSiteValue::try_from(instruction)
                        .ok()
                        .and_then(CallSiteValue::get_called_fn_value)
                        .is_some_and(|callee| {
                            callee
                                .get_name()
                                .to_str()
                                .ok()
                                .and_then(ResultOp::from_name)
                                .is_some_and(|op| op.result_operand().is_some())
                        })
                })
            })
        })
}

pub fn initialize_static_results<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    entry: FunctionValue<'ctx>,
    dynamic: bool,
) -> Result<(), String> {
    if !uses_static_results(module, entry, dynamic) {
        return Ok(());
    }
    let count = u32::try_from(get_required_num_results(entry)?).map_err(|e| e.to_string())?;
    let ty = get_dynamic_result_slot_type(ctx).array_type(count);
    let table = module.add_global(ty, None, STATIC_RESULTS);
    table.set_linkage(Linkage::Private);
    table.set_initializer(&ty.const_zero());
    let b = ctx.create_builder();
    let first = entry
        .get_first_basic_block()
        .and_then(inkwell::basic_block::BasicBlock::get_first_instruction)
        .ok_or("Missing entry instructions")?;
    let initialize = module.add_function(
        "qir_qis.init_static_results",
        ctx.void_type().fn_type(&[], false),
        Some(Linkage::Private),
    );
    b.position_at_end(ctx.append_basic_block(initialize, "entry"));
    // An aggregate zero store expands into hundreds of thousands of scalar
    // stores in downstream -O0 codegen at the static-result resource limit.
    b.build_memset(
        table.as_pointer_value(),
        1,
        ctx.i8_type().const_zero(),
        ty.size_of()
            .ok_or("Static result table must have a sized LLVM type")?,
    )
    .map_err(|e| e.to_string())?;
    b.build_return(None).map_err(|e| e.to_string())?;
    b.position_before(&first);
    b.build_call(initialize, &[], "")
        .map_err(|e| e.to_string())?;
    // One shared table permits result operations in helpers without changing their ABI.
    let cleanup = module.add_function(
        "qir_qis.clear_static_results",
        ctx.void_type().fn_type(&[], false),
        Some(Linkage::Private),
    );
    b.position_at_end(ctx.append_basic_block(cleanup, "entry"));
    build_dynamic_array_loop(
        ctx,
        cleanup,
        &b,
        ctx.i64_type().const_int(u64::from(count), false),
        |b, i| {
            let slot = unsafe {
                b.build_gep(
                    ty,
                    table.as_pointer_value(),
                    &[ctx.i64_type().const_zero(), i],
                    "slot",
                )
            }
            .map_err(|e| e.to_string())?;
            b.build_call(ensure_result_slot_clear(ctx, module), &[slot.into()], "")
                .map_err(|e| e.to_string())?;
            Ok(())
        },
    )?;
    b.build_return(None).map_err(|e| e.to_string())?;
    for bb in entry.get_basic_blocks() {
        if let Some(ret) = bb
            .get_terminator()
            .filter(|i| i.get_opcode() == inkwell::values::InstructionOpcode::Return)
        {
            b.position_before(&ret);
            b.build_call(cleanup, &[], "").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn result_slot_pointer<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder<'ctx>,
    result: PointerValue<'ctx>,
) -> Result<PointerValue<'ctx>, String> {
    if module.get_global(STATIC_RESULTS).is_none() {
        return Ok(result);
    }
    let lookup = ensure_static_result_lookup(ctx, module);
    Ok(builder
        .build_call(lookup, &[result.into()], "result_slot")
        .map_err(|e| e.to_string())?
        .try_as_basic_value()
        .basic()
        .ok_or("Missing static result slot")?
        .into_pointer_value())
}

fn ensure_static_result_lookup<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(f) = module.get_function("qir_qis.static_result_slot") {
        return f;
    }
    let table = module
        .get_global(STATIC_RESULTS)
        .expect("static result table");
    let ty = table.get_value_type().into_array_type();
    let ptr = ctx.ptr_type(AddressSpace::default());
    let f = module.add_function(
        "qir_qis.static_result_slot",
        ptr.fn_type(&[ptr.into()], false),
        Some(Linkage::Private),
    );
    let b = ctx.create_builder();
    b.position_at_end(ctx.append_basic_block(f, "entry"));
    let valid = ctx.append_basic_block(f, "valid");
    let invalid = ctx.append_basic_block(f, "invalid");
    let id = b
        .build_ptr_to_int(
            f.get_first_param().expect("result ID").into_pointer_value(),
            ctx.i64_type(),
            "id",
        )
        .expect("decode ID");
    let in_range = b
        .build_int_compare(
            IntPredicate::ULT,
            id,
            ctx.i64_type().const_int(u64::from(ty.len()), false),
            "in_range",
        )
        .expect("bounds check");
    b.build_conditional_branch(in_range, valid, invalid)
        .expect("checked lookup");
    b.position_at_end(invalid);
    build_panic_with_message(
        ctx,
        module,
        &b,
        "qir_qis.static_result_bounds_error",
        "Static result index out of bounds.",
    )
    .expect("bounds panic message");
    b.position_at_end(valid);
    let slot = unsafe {
        b.build_gep(
            ty,
            table.as_pointer_value(),
            &[ctx.i64_type().const_zero(), id],
            "slot",
        )
    }
    .expect("static slot pointer");
    b.build_return(Some(&slot)).expect("return static slot");
    f
}

pub fn result_read_function<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
    dynamic: bool,
) -> Option<FunctionValue<'ctx>> {
    if module.get_global(STATIC_RESULTS).is_none() {
        return dynamic.then(|| ensure_dynamic_result_read(ctx, module));
    }
    if let Some(f) = module.get_function("qir_qis.static_result_read") {
        return Some(f);
    }
    let ptr = ctx.ptr_type(AddressSpace::default());
    let f = module.add_function(
        "qir_qis.static_result_read",
        ctx.bool_type().fn_type(&[ptr.into()], false),
        Some(Linkage::Private),
    );
    let b = ctx.create_builder();
    b.position_at_end(ctx.append_basic_block(f, "entry"));
    let slot = result_slot_pointer(
        ctx,
        module,
        &b,
        f.get_first_param().expect("result ID").into_pointer_value(),
    )
    .expect("static slot lookup");
    let value = b
        .build_call(
            ensure_dynamic_result_read(ctx, module),
            &[slot.into()],
            "value",
        )
        .expect("read static slot")
        .try_as_basic_value()
        .basic()
        .expect("result bool");
    b.build_return(Some(&value)).expect("return bool");
    Some(f)
}

#[cfg(test)]
mod tests;
