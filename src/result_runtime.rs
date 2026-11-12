//! Result storage shared by scalar and array lowering.
//!
//! Static IDs index a private table; dynamic IDs own heap slots. Keeping slot
//! cleanup separate from deallocation lets both representations share futures.
#![allow(
    clippy::expect_used,
    reason = "builder operations construct fixed, internally checked LLVM helper layouts"
)]

use inkwell::{
    AddressSpace, IntPredicate,
    attributes::AttributeLoc,
    builder::Builder,
    context::Context,
    module::{Linkage, Module},
    values::{BasicValue, CallSiteValue, FunctionValue, PointerValue},
};

use crate::{
    aux::{
        build_dynamic_array_loop, ensure_dynamic_result_array_release, ensure_dynamic_result_read,
        ensure_out_err_success, ensure_result_slot_clear, get_dynamic_result_slot_type,
        initialize_dynamic_result_slot,
    },
    convert::{build_panic_with_message, get_or_create_function, get_required_num_results},
};

pub const STATIC_RESULTS: &str = "qir_qis.static_results";

pub fn validate_input_symbols(ctx: &Context, module: &Module<'_>) -> Result<(), String> {
    validate_alias_symbols(module)?;
    validate_heap_runtime_symbols(module)?;
    for global in module.get_globals() {
        if global.get_name().to_bytes().starts_with(b"qir_qis.") {
            return Err("Input QIR must not define internal result storage globals".into());
        }
    }
    for function in module.get_functions() {
        if matches!(
            function.get_name().to_str(),
            Ok("__quantum__rt__result_allocate"
                | "__quantum__rt__result_release"
                | "__quantum__rt__result_array_allocate"
                | "__quantum__rt__result_array_release"
                | "__quantum__rt__result_array_record_output"
                | "__quantum__rt__read_result"
                | "__quantum__rt__result_record_output"
                | "__quantum__qis__mz__body"
                | "__quantum__qis__m__body"
                | "__quantum__qis__mresetz__body")
        ) {
            crate::aux::validate_result_signature(
                function.get_name().to_str().map_err(|e| e.to_string())?,
                function.get_type(),
            )?;
        }
        if function.get_name().to_bytes().starts_with(b"qir_qis.") {
            return Err(format!(
                "Input QIR must not define internal helper function: {}",
                function.get_name().to_string_lossy()
            ));
        }
    }
    let ptr = ctx.ptr_type(AddressSpace::default());
    let panic_type = ctx
        .void_type()
        .fn_type(&[ctx.i32_type().into(), ptr.into()], false);
    if module.get_global("panic").is_some()
        || module
            .get_function("panic")
            .is_some_and(|f| f.get_type() != panic_type || f.count_basic_blocks() != 0)
    {
        return Err("Invalid downstream runtime declaration: panic".into());
    }
    validate_output_tags(module)
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

fn validate_output_tags(module: &Module<'_>) -> Result<(), String> {
    for function in module.get_functions() {
        for block in function.get_basic_blocks() {
            for instruction in block.get_instructions() {
                let Ok(call) = CallSiteValue::try_from(instruction) else {
                    continue;
                };
                let Some(callee) = call.get_called_fn_value() else {
                    continue;
                };
                let index = match callee.get_name().to_str() {
                    Ok("__quantum__rt__result_record_output") => 1,
                    Ok("__quantum__rt__result_array_record_output") => 2,
                    _ => continue,
                };
                let tag = instruction
                    .get_operand(index)
                    .and_then(inkwell::values::Operand::value)
                    .ok_or("Missing result output tag")?;
                if crate::convert::resolve_output_tag(tag)?.is_none() && index == 2 {
                    return Err("Result array output requires a constant global tag".into());
                }
            }
        }
    }
    Ok(())
}

fn validate_alias_symbols(module: &Module<'_>) -> Result<(), String> {
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
                    || [b"heap_alloc".as_slice(), b"heap_free", b"panic"].contains(&name)
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
    let mut errors = Vec::new();
    crate::aux::validate_dynamic_array_allocation_backing(module, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

pub fn uses_static_results(module: &Module<'_>, dynamic: bool) -> bool {
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
            function
                .get_string_attribute(AttributeLoc::Function, "entry_point")
                .is_none()
                && !function.get_name().to_bytes().starts_with(b"qir_qis.")
        })
        .any(|function| {
            function.get_basic_blocks().into_iter().any(|block| {
                block.get_instructions().any(|instruction| {
                    CallSiteValue::try_from(instruction)
                        .ok()
                        .and_then(CallSiteValue::get_called_fn_value)
                        .is_some_and(|callee| {
                            matches!(
                                callee.get_name().to_str(),
                                Ok("__quantum__qis__mz__body"
                                    | "__quantum__qis__m__body"
                                    | "__quantum__qis__mresetz__body"
                                    | "__quantum__rt__read_result"
                                    | "__quantum__rt__result_record_output")
                            )
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
    if !uses_static_results(module, dynamic) {
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
