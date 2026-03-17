//! QIR Decomposition Module
//!
//! This module provides functionality for decomposing high-level quantum gates into
//! a native gate set consisting of RXY, RZ, and RZZ gates. The decomposition process
//! works by:
//!
//! 1. Creating a separate LLVM module with the decomposition implementations
//! 2. Merging this module into the main QIR module
//! 3. Running LLVM's function inlining pass to replace high-level gates with their implementations
//!
//! ## Gate Decompositions
//! See table at
//! <https://github.com/quantinuum/qir-qis/blob/main/qtm-qir-reference.md#decompositions>

use inkwell::module::{Linkage, Module};
use inkwell::types::PointerType;
use inkwell::{builder::Builder, context::Context, values::FunctionValue};
use std::f64::consts::PI;

pub struct QirTypes<'ctx> {
    pub qubit_ptr_type: PointerType<'ctx>,
}

impl<'ctx> QirTypes<'ctx> {
    #[must_use]
    pub fn new(context: &'ctx Context) -> Self {
        let qubit_ptr_type = context.ptr_type(inkwell::AddressSpace::default());
        QirTypes { qubit_ptr_type }
    }
}

/// Struct to hold native QIR gates
struct NativeGates<'ctx> {
    rxy: FunctionValue<'ctx>,
    rz: FunctionValue<'ctx>,
    rzz: FunctionValue<'ctx>,
}

/// Adds QIR decompositions to the given module.
/// # Errors
/// Returns an error if the module verification fails.
pub fn add_decompositions<'ctx>(ctx: &'ctx Context, module: &Module<'ctx>) -> Result<(), String> {
    build_decompositions(ctx, module)?;
    crate::llvm_verify::verify_module(module, "Module verification failed")?;

    Ok(())
}

/// Builds the QIR decompositions for various quantum gates.
fn build_decompositions<'ctx>(context: &'ctx Context, module: &Module<'ctx>) -> Result<(), String> {
    let qir_types = QirTypes::new(context);
    let builder = context.create_builder();

    let rxy = declare_rxy(context, module, &qir_types);
    let rz = declare_rz(context, module, &qir_types);
    let rzz = declare_rzz(context, module, &qir_types);
    let native_gates = NativeGates { rxy, rz, rzz };

    // Single-qubit gates
    define_h_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_x_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_y_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_z_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_s_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_s_adj_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_t_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_t_adj_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_rx_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_ry_gate(context, module, &builder, &qir_types, &native_gates)?;

    // Two-qubit gates
    define_cz_gate(context, module, &builder, &qir_types, &native_gates)?;
    define_cx_gate(context, module, &builder, &qir_types, &native_gates, "cx")?;
    // Legacy: Combine with above if we deprecate CNOT
    define_cx_gate(context, module, &builder, &qir_types, &native_gates, "cnot")?;

    // Three-qubit gate
    define_ccx_gate(context, module, &builder, &qir_types, &native_gates)?;
    Ok(())
}

/// Declare the native QIR gate rxy
fn declare_rxy<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    qir_types: &QirTypes<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function("__quantum__qis__rxy__body") {
        return existing;
    }
    let fn_type = context.void_type().fn_type(
        &[
            context.f64_type().into(),
            context.f64_type().into(),
            qir_types.qubit_ptr_type.into(),
        ],
        false,
    );
    module.add_function(
        "__quantum__qis__rxy__body",
        fn_type,
        Some(Linkage::External),
    )
}

/// Declare the native QIR gate rz
fn declare_rz<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    qir_types: &QirTypes<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function("__quantum__qis__rz__body") {
        return existing;
    }
    let fn_type = context.void_type().fn_type(
        &[context.f64_type().into(), qir_types.qubit_ptr_type.into()],
        false,
    );
    module.add_function("__quantum__qis__rz__body", fn_type, Some(Linkage::External))
}

/// Declare the native QIR gate rzz
fn declare_rzz<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    qir_types: &QirTypes<'ctx>,
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function("__quantum__qis__rzz__body") {
        return existing;
    }
    let fn_type = context.void_type().fn_type(
        &[
            context.f64_type().into(),
            qir_types.qubit_ptr_type.into(),
            qir_types.qubit_ptr_type.into(),
        ],
        false,
    );
    module.add_function(
        "__quantum__qis__rzz__body",
        fn_type,
        Some(Linkage::External),
    )
}

fn get_or_create_decomposition_function<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    fn_name: &str,
    param_types: &[inkwell::types::BasicMetadataTypeEnum<'ctx>],
) -> FunctionValue<'ctx> {
    if let Some(existing) = module.get_function(fn_name) {
        return existing;
    }
    let fn_type = context.void_type().fn_type(param_types, false);
    module.add_function(fn_name, fn_type, Some(Linkage::LinkOnceODR))
}

/// Define decomposition of H gate using native gates
/// # Errors
/// Returns an error if the function parameters are invalid.
fn define_h_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let h = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__h__body",
        &[qir_types.qubit_ptr_type.into()],
    );
    if h.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(h, "entry");
    builder.position_at_end(entry);

    let qubit = h
        .get_first_param()
        .ok_or("H gate function missing first parameter")?
        .into_pointer_value();

    let pi = context.f64_type().const_float(PI);
    let half_pi = context.f64_type().const_float(PI / 2.0);
    let neg_half_pi = context.f64_type().const_float(PI / -2.0);

    // rxy(π/2, -π/2, qubit)
    let _ = builder.build_call(
        native.rxy,
        &[half_pi.into(), neg_half_pi.into(), qubit.into()],
        "",
    );

    // rz(π, qubit)
    let _ = builder.build_call(native.rz, &[pi.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of X gate using native gates
fn define_x_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let x = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__x__body",
        &[qir_types.qubit_ptr_type.into()],
    );
    if x.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(x, "entry");
    builder.position_at_end(entry);

    let qubit = x
        .get_first_param()
        .ok_or("X gate function missing first parameter")?
        .into_pointer_value();

    let pi = context.f64_type().const_float(PI);
    let zero = context.f64_type().const_zero();

    // rxy(π, 0, qubit)
    let _ = builder.build_call(native.rxy, &[pi.into(), zero.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of Y gate using native gates
fn define_y_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let y = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__y__body",
        &[qir_types.qubit_ptr_type.into()],
    );
    if y.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(y, "entry");
    builder.position_at_end(entry);

    let qubit = y
        .get_first_param()
        .ok_or("Y gate function missing first parameter")?
        .into_pointer_value();

    let pi = context.f64_type().const_float(PI);
    let half_pi = context.f64_type().const_float(PI / 2.0);

    // rxy(π, π/2, qubit)
    let _ = builder.build_call(native.rxy, &[pi.into(), half_pi.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of Z gate using native gates
fn define_z_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let z = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__z__body",
        &[qir_types.qubit_ptr_type.into()],
    );
    if z.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(z, "entry");
    builder.position_at_end(entry);

    let qubit = z
        .get_first_param()
        .ok_or("Z gate function missing first parameter")?
        .into_pointer_value();

    let pi = context.f64_type().const_float(PI);

    // rz(π, qubit)
    let _ = builder.build_call(native.rz, &[pi.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of S gate using native gates
fn define_s_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let s = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__s__body",
        &[qir_types.qubit_ptr_type.into()],
    );
    if s.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(s, "entry");
    builder.position_at_end(entry);

    let qubit = s
        .get_first_param()
        .ok_or("S gate function missing first parameter")?
        .into_pointer_value();

    let half_pi = context.f64_type().const_float(PI / 2.0);

    // rz(π/2, qubit)
    let _ = builder.build_call(native.rz, &[half_pi.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of S† gate using native gates
fn define_s_adj_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let s = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__s__adj",
        &[qir_types.qubit_ptr_type.into()],
    );
    if s.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(s, "entry");
    builder.position_at_end(entry);

    let qubit = s
        .get_first_param()
        .ok_or("S_adj gate function missing first parameter")?
        .into_pointer_value();

    let neg_half_pi = context.f64_type().const_float(PI / -2.0);

    // rz(-π/2, qubit)
    let _ = builder.build_call(native.rz, &[neg_half_pi.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of T gate using native gates
fn define_t_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let t = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__t__body",
        &[qir_types.qubit_ptr_type.into()],
    );
    if t.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(t, "entry");
    builder.position_at_end(entry);

    let qubit = t
        .get_first_param()
        .ok_or("T gate function missing first parameter")?
        .into_pointer_value();

    let quarter_pi = context.f64_type().const_float(PI / 4.0);

    // rz(π/4, qubit)
    let _ = builder.build_call(native.rz, &[quarter_pi.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of T† gate using native gates
fn define_t_adj_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let t = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__t__adj",
        &[qir_types.qubit_ptr_type.into()],
    );
    if t.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(t, "entry");
    builder.position_at_end(entry);

    let qubit = t
        .get_first_param()
        .ok_or("T_adj gate function missing first parameter")?
        .into_pointer_value();

    let neg_quarter_pi = context.f64_type().const_float(PI / -4.0);

    // rz(-π/4, qubit)
    let _ = builder.build_call(native.rz, &[neg_quarter_pi.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of RX gate using native gates
fn define_rx_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let rx = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__rx__body",
        &[context.f64_type().into(), qir_types.qubit_ptr_type.into()],
    );
    if rx.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(rx, "entry");
    builder.position_at_end(entry);

    let angle = rx
        .get_first_param()
        .ok_or("RX gate function missing first parameter")?;
    let qubit = rx
        .get_nth_param(1)
        .ok_or("RX gate function missing second parameter")?
        .into_pointer_value();

    let zero = context.f64_type().const_zero();

    // rxy(angle, 0, qubit)
    let _ = builder.build_call(native.rxy, &[angle.into(), zero.into(), qubit.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of RY gate using native gates
fn define_ry_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let ry = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__ry__body",
        &[context.f64_type().into(), qir_types.qubit_ptr_type.into()],
    );
    if ry.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(ry, "entry");
    builder.position_at_end(entry);

    let angle = ry
        .get_first_param()
        .ok_or("RY gate function missing first parameter")?;
    let qubit = ry
        .get_nth_param(1)
        .ok_or("RY gate function missing second parameter")?
        .into_pointer_value();

    let half_pi = context.f64_type().const_float(PI / 2.0);

    // rxy(angle, π/2, qubit)
    let _ = builder.build_call(
        native.rxy,
        &[angle.into(), half_pi.into(), qubit.into()],
        "",
    );

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of CZ gate using native gates
fn define_cz_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let cz = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__cz__body",
        &[
            qir_types.qubit_ptr_type.into(),
            qir_types.qubit_ptr_type.into(),
        ],
    );
    if cz.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(cz, "entry");
    builder.position_at_end(entry);

    let control = cz
        .get_first_param()
        .ok_or("CZ gate function missing first parameter")?
        .into_pointer_value();
    let target = cz
        .get_nth_param(1)
        .ok_or("CZ gate function missing second parameter")?
        .into_pointer_value();

    let half_pi = context.f64_type().const_float(PI / 2.0);
    let neg_half_pi = context.f64_type().const_float(PI / -2.0);

    // rzz(π/2, control, target)
    let _ = builder.build_call(
        native.rzz,
        &[half_pi.into(), control.into(), target.into()],
        "",
    );

    // rz(-π/2, target)
    let _ = builder.build_call(native.rz, &[neg_half_pi.into(), target.into()], "");

    // rz(-π/2, control)
    let _ = builder.build_call(native.rz, &[neg_half_pi.into(), control.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of CX gate using native gates
fn define_cx_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
    fn_name: &str,
) -> Result<(), String> {
    let gate_symbol = format!("__quantum__qis__{fn_name}__body");
    let cx = get_or_create_decomposition_function(
        context,
        module,
        &gate_symbol,
        &[
            qir_types.qubit_ptr_type.into(),
            qir_types.qubit_ptr_type.into(),
        ],
    );
    if cx.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(cx, "entry");
    builder.position_at_end(entry);

    let control = cx
        .get_first_param()
        .ok_or("CX gate function missing first parameter")?
        .into_pointer_value();
    let target = cx
        .get_nth_param(1)
        .ok_or("CX gate function missing second parameter")?
        .into_pointer_value();

    let pi = context.f64_type().const_float(PI);
    let half_pi = context.f64_type().const_float(PI / 2.0);
    let neg_half_pi = context.f64_type().const_float(PI / -2.0);

    // rxy(-π/2, π/2, target)
    let _ = builder.build_call(
        native.rxy,
        &[neg_half_pi.into(), half_pi.into(), target.into()],
        "",
    );

    // rzz(π/2, control, target)
    let _ = builder.build_call(
        native.rzz,
        &[half_pi.into(), control.into(), target.into()],
        "",
    );

    // rz(-π/2, control)
    let _ = builder.build_call(native.rz, &[neg_half_pi.into(), control.into()], "");

    // rxy(π/2, π, target)
    let _ = builder.build_call(native.rxy, &[half_pi.into(), pi.into(), target.into()], "");

    // rz(-π/2, target)
    let _ = builder.build_call(native.rz, &[neg_half_pi.into(), target.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}

/// Define decomposition of CCX gate using native gates
#[allow(clippy::too_many_lines)]
fn define_ccx_gate<'ctx>(
    context: &'ctx Context,
    module: &Module<'ctx>,
    builder: &Builder,
    qir_types: &QirTypes<'ctx>,
    native: &NativeGates<'ctx>,
) -> Result<(), String> {
    let ccx = get_or_create_decomposition_function(
        context,
        module,
        "__quantum__qis__ccx__body",
        &[
            qir_types.qubit_ptr_type.into(),
            qir_types.qubit_ptr_type.into(),
            qir_types.qubit_ptr_type.into(),
        ],
    );
    if ccx.get_first_basic_block().is_some() {
        return Ok(());
    }
    let entry = context.append_basic_block(ccx, "entry");
    builder.position_at_end(entry);

    let control1 = ccx
        .get_first_param()
        .ok_or("CCX gate function missing first parameter")?
        .into_pointer_value();
    let control2 = ccx
        .get_nth_param(1)
        .ok_or("CCX gate function missing second parameter")?
        .into_pointer_value();
    let target = ccx
        .get_nth_param(2)
        .ok_or("CCX gate function missing third parameter")?
        .into_pointer_value();

    let pi = context.f64_type().const_float(PI);
    let half_pi = context.f64_type().const_float(PI / 2.0);
    let neg_half_pi = context.f64_type().const_float(PI / -2.0);
    let quarter_pi = context.f64_type().const_float(PI / 4.0);
    let neg_quarter_pi = context.f64_type().const_float(PI / -4.0);
    let neg_three_quarter_pi = context.f64_type().const_float(3.0 * PI / -4.0);
    let zero = context.f64_type().const_zero();

    // rxy(π, -π/2, target)
    let _ = builder.build_call(
        native.rxy,
        &[pi.into(), neg_half_pi.into(), target.into()],
        "",
    );
    // rzz(π/2, control2, target)
    let _ = builder.build_call(
        native.rzz,
        &[half_pi.into(), control2.into(), target.into()],
        "",
    );
    // rxy(π/4, π/2, target)
    let _ = builder.build_call(
        native.rxy,
        &[quarter_pi.into(), half_pi.into(), target.into()],
        "",
    );
    // rzz(π/2, control1, target)
    let _ = builder.build_call(
        native.rzz,
        &[half_pi.into(), control1.into(), target.into()],
        "",
    );
    // rxy(π/4, 0, target)
    let _ = builder.build_call(
        native.rxy,
        &[quarter_pi.into(), zero.into(), target.into()],
        "",
    );
    // rzz(π/2, control2, target)
    let _ = builder.build_call(
        native.rzz,
        &[half_pi.into(), control2.into(), target.into()],
        "",
    );
    // rxy(π/4, -π/2, target)
    let _ = builder.build_call(
        native.rxy,
        &[quarter_pi.into(), neg_half_pi.into(), target.into()],
        "",
    );
    // rzz(π/2, control1, target)
    let _ = builder.build_call(
        native.rzz,
        &[half_pi.into(), control1.into(), target.into()],
        "",
    );
    // rxy(π, π/4, control1)
    let _ = builder.build_call(
        native.rxy,
        &[pi.into(), quarter_pi.into(), control1.into()],
        "",
    );
    // rxy(-3π/4, π, target)
    let _ = builder.build_call(
        native.rxy,
        &[neg_three_quarter_pi.into(), pi.into(), target.into()],
        "",
    );
    // rzz(π/4, control1, control2)
    let _ = builder.build_call(
        native.rzz,
        &[quarter_pi.into(), control1.into(), control2.into()],
        "",
    );
    // rz(π, target)
    let _ = builder.build_call(native.rz, &[pi.into(), target.into()], "");
    // rxy(π, -π/4, control1)
    let _ = builder.build_call(
        native.rxy,
        &[pi.into(), neg_quarter_pi.into(), control1.into()],
        "",
    );
    // rz(-3π/4, control2)
    let _ = builder.build_call(
        native.rz,
        &[neg_three_quarter_pi.into(), control2.into()],
        "",
    );
    // rz(π/4, control1)
    let _ = builder.build_call(native.rz, &[quarter_pi.into(), control1.into()], "");

    let _ = builder.build_return(None);
    Ok(())
}
