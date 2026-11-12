//! Execution and conformance regressions for QIR result storage.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test failures should report the violated contract"
)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "small deterministic test workloads and native callback ABI conversions"
)]

use crate::{qir_ll_to_bc, qir_to_qis, validate_qir};
use inkwell::{
    OptimizationLevel,
    context::Context,
    memory_buffer::MemoryBuffer,
    module::Module,
    targets::{InitializationConfig, Target},
};
use std::{
    alloc::{Layout, alloc, dealloc},
    cell::RefCell,
    collections::HashMap,
    fmt::Write,
    sync::Mutex,
};

static JIT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default, Debug)]
struct Runtime {
    allocations: HashMap<usize, Layout>,
    allocated: usize,
    freed: usize,
    peak: usize,
    fail_after: Option<usize>,
    futures: HashMap<u64, bool>,
    next_future: u64,
    reads: usize,
    decrements: usize,
    next_qubit: u64,
    outputs: Vec<(String, Vec<bool>)>,
    errors: Vec<String>,
}
thread_local! { static RUNTIME: RefCell<Runtime> = RefCell::default(); }

extern "C" fn heap_alloc(size: u64) -> *mut u8 {
    RUNTIME.with_borrow_mut(|rt| {
        if rt.fail_after.is_some_and(|limit| rt.allocated >= limit) {
            return std::ptr::null_mut();
        }
        let layout = Layout::from_size_align(size as usize, 8).unwrap();
        // SAFETY: LLVM asks for the concrete slot size; tracked layouts match dealloc.
        let ptr = unsafe { alloc(layout) };
        if !ptr.is_null() {
            rt.allocations.insert(ptr as usize, layout);
            rt.allocated += 1;
            rt.peak = rt.peak.max(rt.allocations.len());
        }
        ptr
    })
}
extern "C" fn heap_free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    RUNTIME.with_borrow_mut(|rt| {
        if let Some(layout) = rt.allocations.remove(&(ptr as usize)) {
            // SAFETY: this address was allocated with exactly this layout.
            unsafe {
                dealloc(ptr, layout);
            }
            rt.freed += 1;
        } else {
            rt.errors.push("free of unowned slot".into());
        }
    });
}
extern "C" fn measure(qubit: u64) -> u64 {
    RUNTIME.with_borrow_mut(|rt| {
        rt.next_future += 1;
        rt.futures.insert(rt.next_future, qubit % 2 == 1);
        rt.next_future
    })
}
extern "C" fn read_future(future: u64) -> bool {
    RUNTIME.with_borrow_mut(|rt| {
        rt.reads += 1;
        rt.futures.get(&future).copied().unwrap_or_else(|| {
            rt.errors.push("read of dead future".into());
            false
        })
    })
}
extern "C" fn decrement(future: u64) {
    RUNTIME.with_borrow_mut(|rt| {
        rt.decrements += 1;
        if rt.futures.remove(&future).is_none() {
            rt.errors.push("double decrement".into());
        }
    });
}
extern "C" fn qubit_allocate() -> u64 {
    RUNTIME.with_borrow_mut(|rt| {
        let q = rt.next_qubit;
        rt.next_qubit += 1;
        q
    })
}
extern "C" fn runtime_panic(code: u32, message: *const u8) {
    // SAFETY: generated panic strings carry the exact payload length in their first byte.
    let length = unsafe { message.read() };
    let diagnostic = label(message, u64::from(length));
    eprintln!("diagnostic: {diagnostic}");
    let live = RUNTIME.with_borrow(|rt| rt.allocations.len());
    eprintln!("qir-qis runtime panic: {code}, live slots: {live}");
    std::process::exit(77);
}
extern "C" fn noop_qubit(_: u64) {}
extern "C" fn setup(_: u64) {}
extern "C" fn teardown() -> u64 {
    0
}
extern "C" fn rxy(_: u64, _: f64, _: f64) {}
extern "C" fn rz(_: u64, _: f64) {}
extern "C" fn rzz(_: u64, _: u64, _: f64) {}
fn label(ptr: *const u8, len: u64) -> String {
    // SAFETY: print calls pass a length-prefixed compiler-generated string.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ptr.add(1), len as usize) })
        .into_owned()
}
extern "C" fn print_bool(tag: *const u8, len: u64, value: bool) {
    RUNTIME.with_borrow_mut(|rt| rt.outputs.push((label(tag, len), vec![value])));
}
#[repr(C, packed)]
struct BoolArray {
    len: i32,
    rank: i32,
    data: *const u8,
    mask: *const i32,
}
extern "C" fn print_array(tag: *const u8, len: u64, array: *const BoolArray) {
    // SAFETY: the compiler passes a packed descriptor and initialized i1 storage.
    let descriptor = unsafe { array.read_unaligned() };
    let values = unsafe { std::slice::from_raw_parts(descriptor.data, descriptor.len as usize) }
        .iter()
        .map(|v| *v != 0)
        .collect();
    RUNTIME.with_borrow_mut(|rt| rt.outputs.push((label(tag, len), values)));
}

fn qir(body: &str, helpers: &str, dynamic: bool, arrays: bool) -> String {
    format!(
        r#"
@tag = private constant [4 x i8] c"out\00"
define i64 @entry() #0 {{
{body}
}}
{helpers}
declare ptr @__quantum__rt__result_allocate(ptr)
declare void @__quantum__rt__result_release(ptr)
declare void @__quantum__rt__result_array_allocate(i64, ptr, ptr)
declare void @__quantum__rt__result_array_release(i64, ptr)
declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)
declare void @__quantum__rt__result_record_output(ptr, ptr)
declare i1 @__quantum__rt__read_result(ptr)
declare void @__quantum__qis__mz__body(ptr, ptr) #1
attributes #0 = {{ "entry_point" "qir_profiles"="adaptive_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="2" "required_num_results"="2" }}
attributes #1 = {{ "irreversible" }}
!llvm.module.flags = !{{!0, !1, !2, !3, !4, !5, !6}}
!0 = !{{i32 1, !"qir_major_version", i32 2}}
!1 = !{{i32 7, !"qir_minor_version", i32 1}}
!2 = !{{i32 1, !"dynamic_qubit_management", i1 false}}
!3 = !{{i32 1, !"dynamic_result_management", i1 {dynamic}}}
!4 = !{{i32 1, !"arrays", i1 {arrays}}}
!5 = !{{i32 1, !"ir_functions", i1 true}}
!6 = !{{i32 1, !"backwards_branching", i1 true}}
"#
    )
}

fn execute(ll: &str, optimization: u32, fail_after: Option<usize>, expected_exit: u64) -> Runtime {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    RUNTIME.with_borrow_mut(|rt| {
        *rt = Runtime {
            fail_after,
            ..Runtime::default()
        }
    });
    let bc = qir_ll_to_bc(ll).expect("parse QIR");
    validate_qir(&bc, None).expect("spec-valid QIR");
    let target = if cfg!(windows) && optimization > 0 {
        "x86-64"
    } else {
        "native"
    };
    let compiled = qir_to_qis(&bc, optimization, target, None).expect("translate QIR");
    let ctx = Context::create();
    let buffer = MemoryBuffer::create_from_memory_range_copy(&compiled, "qis");
    let module = Module::parse_bitcode_from_buffer(&buffer, &ctx).unwrap();
    module.verify().expect("LLVM-verifiable translation");
    Target::initialize_native(&InitializationConfig::default()).unwrap();
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .unwrap();
    for (name, address) in [
        ("heap_alloc", heap_alloc as *const () as usize),
        ("heap_free", heap_free as *const () as usize),
        ("___lazy_measure", measure as *const () as usize),
        ("___read_future_bool", read_future as *const () as usize),
        ("___dec_future_refcount", decrement as *const () as usize),
        ("___qalloc", qubit_allocate as *const () as usize),
        ("___qfree", noop_qubit as *const () as usize),
        ("___reset", noop_qubit as *const () as usize),
        ("print_bool", print_bool as *const () as usize),
        ("print_bool_arr", print_array as *const () as usize),
        ("___rxy", rxy as *const () as usize),
        ("___rz", rz as *const () as usize),
        ("___rzz", rzz as *const () as usize),
        ("panic", runtime_panic as *const () as usize),
        ("setup", setup as *const () as usize),
        ("teardown", teardown as *const () as usize),
    ] {
        if let Some(function) = module.get_function(name) {
            engine.add_global_mapping(&function, address);
        }
    }
    // SAFETY: the fixture entry has exactly the declared C ABI and runtime stubs.
    let result = unsafe {
        engine
            .get_function::<unsafe extern "C" fn() -> u64>("___user_qir_entry")
            .unwrap()
            .call()
    };
    assert_eq!(result, expected_exit);
    let runtime = RUNTIME.with_borrow_mut(std::mem::take);
    assert!(runtime.errors.is_empty(), "{:?}", runtime.errors);
    assert!(
        runtime.allocations.is_empty(),
        "leaked result slots: {runtime:?}"
    );
    assert!(runtime.futures.is_empty(), "leaked futures: {runtime:?}");
    runtime
}

#[test]
fn result_capabilities_follow_spec_matrix() {
    // QIR Memory_Management.md: array output is independent of dynamic allocation.
    // https://github.com/qir-alliance/qir-spec/blob/b5e157ca399538f83f570fb49eab5fa5db474c83/specification/Memory_Management.md
    for arrays in [false, true] {
        for dynamic in [false, true] {
            for (call, expected) in [
                (
                    "call void @__quantum__rt__result_array_record_output(i64 1, ptr %a, ptr @tag)",
                    arrays,
                ),
                (
                    "%r = call ptr @__quantum__rt__result_allocate(ptr null)",
                    dynamic,
                ),
                (
                    "call void @__quantum__rt__result_release(ptr null)",
                    dynamic,
                ),
                (
                    "call void @__quantum__rt__result_array_allocate(i64 1, ptr %a, ptr null)",
                    arrays && dynamic,
                ),
                (
                    "call void @__quantum__rt__result_array_release(i64 1, ptr %a)",
                    arrays && dynamic,
                ),
            ] {
                let ll = qir(
                    &format!("entry:\n %a = alloca [1 x ptr]\n {call}\n ret i64 0"),
                    "",
                    dynamic,
                    arrays,
                );
                assert_eq!(
                    validate_qir(&qir_ll_to_bc(&ll).unwrap(), None).is_ok(),
                    expected,
                    "{call}, arrays={arrays}, dynamic={dynamic}"
                );
                assert_eq!(
                    qir_to_qis(&qir_ll_to_bc(&ll).unwrap(), 0, "native", None).is_ok(),
                    expected,
                    "direct translation: {call}, arrays={arrays}, dynamic={dynamic}"
                );
            }
        }
    }
}

#[test]
fn static_arrays_preserve_values_order_duplicates_and_cached_reads() {
    let ll = qir(
        r"entry:
  %a = alloca [3 x ptr]
  %v0 = insertvalue [3 x ptr] poison, ptr inttoptr (i64 1 to ptr), 0
  %v1 = insertvalue [3 x ptr] %v0, ptr null, 1
  %v2 = insertvalue [3 x ptr] %v1, ptr inttoptr (i64 1 to ptr), 2
  store [3 x ptr] %v2, ptr %a
  call void @__quantum__qis__mz__body(ptr null, ptr null)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 1 to ptr))
  %cached = call i1 @__quantum__rt__read_result(ptr inttoptr (i64 1 to ptr))
  call void @__quantum__rt__result_array_record_output(i64 3, ptr %a, ptr @tag)
  call void @__quantum__rt__result_record_output(ptr null, ptr @tag)
  call void @__quantum__rt__result_array_record_output(i64 3, ptr %a, ptr @tag)
  ret i64 0",
        "",
        false,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 0);
        assert_eq!(
            rt.outputs,
            vec![
                ("USER:RESULT_ARRAY:out".into(), vec![true, false, true]),
                ("USER:RESULT:out".into(), vec![false]),
                ("USER:RESULT_ARRAY:out".into(), vec![true, false, true]),
            ]
        );
        assert_eq!((rt.reads, rt.decrements, rt.allocated), (2, 2, 0));
    }
}

#[test]
fn helper_allocations_escape_and_remain_distinct() {
    let ll = qir(
        r"entry:
  %a = alloca [2 x ptr]
  br label %body
body:
  call void @allocate(ptr %a)
  %r0 = load ptr, ptr %a
  %second = getelementptr [2 x ptr], ptr %a, i64 0, i64 1
  call void @allocate(ptr %second)
  %r1 = load ptr, ptr %second
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr %r0)
  call void @__quantum__qis__mz__body(ptr null, ptr %r1)
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %a, ptr @tag)
  call void @__quantum__rt__result_array_release(i64 2, ptr %a)
  ret i64 0",
        r"define void @allocate(ptr %out) {
entry:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  store ptr %r, ptr %out
  ret void
}",
        true,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 0);
        assert_eq!(rt.outputs.first().expect("one output").1, vec![true, false]);
        assert_eq!((rt.allocated, rt.freed, rt.peak), (2, 2, 2));
    }
}

#[test]
fn loops_reclaim_scalar_and_bulk_storage_and_pending_futures() {
    let ll = qir(
        r"entry:
  %a = alloca [2 x ptr]
  br label %header
header:
  %i = phi i64 [0, %entry], [%next, %body]
  %more = icmp ult i64 %i, 128
  br i1 %more, label %body, label %done
body:
  call void @work(ptr inttoptr (i64 1 to ptr))
  call void @__quantum__rt__result_array_allocate(i64 2, ptr %a, ptr null)
  %r = load ptr, ptr %a
  call void @__quantum__qis__mz__body(ptr null, ptr %r)
  call void @__quantum__rt__result_array_release(i64 2, ptr %a)
  %next = add i64 %i, 1
  br label %header
done:
  ret i64 0",
        r"define void @work(ptr %q) {
entry:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__qis__mz__body(ptr %q, ptr %r)
  call void @__quantum__qis__mz__body(ptr %q, ptr %r)
  %v = call i1 @__quantum__rt__read_result(ptr %r)
  %cached = call i1 @__quantum__rt__read_result(ptr %r)
  call void @__quantum__rt__result_release(ptr %r)
  ret void
}",
        true,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 0);
        assert_eq!((rt.allocated, rt.freed, rt.peak), (384, 384, 2));
        assert_eq!((rt.reads, rt.decrements), (128, 384));
    }
}

#[test]
fn repeated_loop_callsite_can_keep_multiple_results_alive() {
    let ll = qir(
        r"entry:
  %a = alloca [2 x ptr]
  br label %loop
loop:
  %i = phi i64 [0, %entry], [%next, %loop]
  %out = getelementptr [2 x ptr], ptr %a, i64 0, i64 %i
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  store ptr %r, ptr %out
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr %r)
  %next = add i64 %i, 1
  %more = icmp ult i64 %next, 2
  br i1 %more, label %loop, label %done
done:
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %a, ptr @tag)
  call void @__quantum__rt__result_array_release(i64 2, ptr %a)
  ret i64 0",
        "",
        true,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 0);
        assert_eq!((rt.allocated, rt.freed, rt.peak), (2, 2, 2));
        assert_eq!((rt.reads, rt.decrements), (2, 2));
        assert_eq!(rt.outputs.first().expect("one output").1, vec![true, true]);
    }
}

#[test]
fn allocation_failure_sets_flags_and_rolls_back_bulk_prefix() {
    for limit in [0, 1] {
        let ll = qir(
            r"entry:
  %a = alloca [2 x ptr]
  %err = alloca i1
  store i1 false, ptr %err
  br label %body
body:
  call void @__quantum__rt__result_array_allocate(i64 2, ptr %a, ptr %err)
  %failed = load i1, ptr %err
  %r0 = load ptr, ptr %a
  %null = icmp eq ptr %r0, null
  %okay = and i1 %failed, %null
  %code = zext i1 %okay to i64
  ret i64 %code",
            "",
            true,
            true,
        );
        for level in [0, 2] {
            let rt = execute(&ll, level, Some(limit), 1);
            assert_eq!((rt.allocated, rt.freed), (limit, limit));
        }
    }
    let ll = qir(
        r"entry:
  %err = alloca i1
  %r = call ptr @__quantum__rt__result_allocate(ptr %err)
  %failed = load i1, ptr %err
  %null = icmp eq ptr %r, null
  %okay = and i1 %failed, %null
  %code = zext i1 %okay to i64
  ret i64 %code",
        "",
        true,
        false,
    );
    execute(&ll, 0, Some(0), 1);
}

#[test]
fn allocations_start_false_and_empty_bulk_clears_error_flag() {
    let ll = qir(
        r"entry:
  %empty = alloca [0 x ptr]
  %err = alloca i1
  store i1 true, ptr %err
  call void @__quantum__rt__result_array_allocate(i64 0, ptr %empty, ptr %err)
  %failed = load i1, ptr %err
  call void @__quantum__rt__result_array_record_output(i64 0, ptr %empty, ptr @tag)
  %r = call ptr @__quantum__rt__result_allocate(ptr %err)
  %value = call i1 @__quantum__rt__read_result(ptr %r)
  %allocation_failed = load i1, ptr %err
  %bad = or i1 %failed, %value
  %all_bad = or i1 %bad, %allocation_failed
  call void @__quantum__rt__result_release(ptr %r)
  %code = zext i1 %all_bad to i64
  ret i64 %code",
        "",
        true,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 0);
        assert_eq!((rt.allocated, rt.freed, rt.reads), (1, 1, 0));
        assert_eq!(
            rt.outputs.first().expect("one output").1,
            Vec::<bool>::new()
        );
    }
}

#[test]
fn static_pending_overwrites_and_multiple_returns_cleanup() {
    let ll = qir(
        r"entry:
  %empty = alloca [0 x ptr]
  call void @__quantum__rt__result_array_record_output(i64 0, ptr %empty, ptr @tag)
  call void @__quantum__qis__mz__body(ptr null, ptr null)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr null)
  %v = call i1 @__quantum__rt__read_result(ptr null)
  br i1 %v, label %yes, label %no
yes:
  call void @__quantum__qis__mz__body(ptr null, ptr inttoptr (i64 1 to ptr))
  ret i64 1
no:
  ret i64 0",
        "",
        false,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 1);
        assert_eq!((rt.reads, rt.decrements), (1, 3));
    }
}

#[test]
fn input_heap_runtime_symbols_cannot_bypass_validation() {
    for (name, signature, call) in [
        (
            "heap_alloc",
            "ptr @heap_alloc(i64)",
            "%p = call ptr @heap_alloc(i64 1)",
        ),
        (
            "heap_free",
            "void @heap_free(ptr)",
            "call void @heap_free(ptr inttoptr (i64 1 to ptr))",
        ),
    ] {
        for attribute in ["", "\"wasm\"", "\"cudaq-fnid\"=\"heap\""] {
            for placement in ["unused", "entry", "helper"] {
                let body = match placement {
                    "entry" => format!("entry:\n {call}\n ret i64 0"),
                    "helper" => "entry:\n call void @helper()\n ret i64 0".into(),
                    _ => "entry:\n ret i64 0".into(),
                };
                let mut helpers = format!("declare {signature}");
                if !attribute.is_empty() {
                    write!(helpers, " #2\nattributes #2 = {{ {attribute} }}").unwrap();
                }
                if placement == "helper" {
                    write!(
                        helpers,
                        "\ndefine void @helper() {{\n {call}\n ret void\n}}"
                    )
                    .unwrap();
                }
                let bc = qir_ll_to_bc(&qir(&body, &helpers, true, false)).unwrap();
                let expected = format!("Input QIR uses reserved heap runtime symbol: {name}");
                let validation = validate_qir(&bc, None).unwrap_err();
                let translation = qir_to_qis(&bc, 0, "native", None).unwrap_err();
                assert!(validation.contains(&expected), "{validation}");
                assert!(translation.contains(&expected), "{translation}");
                let passthrough =
                    crate::qir_to_qis_with_passthrough_calls(&bc, 0, "native", None, &[name])
                        .unwrap_err();
                assert!(passthrough.contains("reserved"), "{passthrough}");
            }
        }
    }
}

#[test]
fn malformed_runtime_symbols_fail_validation_and_direct_translation() {
    for declaration in [
        "declare i64 @heap_alloc(i64)",
        "declare ptr @heap_alloc(i32)",
        "declare ptr @heap_alloc(i64, ...)",
        "declare i64 @heap_free(ptr)",
        "declare void @heap_free(i64)",
        "define ptr @heap_alloc(i64 %n) { ret ptr null }",
        "define void @heap_free(ptr %p) { ret void }",
        "@heap_alloc = global i64 0",
        "@heap_free = global i64 0",
        "@qir_qis.static_results = global i64 0",
    ] {
        let ll = qir("entry:\n ret i64 0", declaration, true, false);
        let bc = qir_ll_to_bc(&ll).unwrap();
        assert!(validate_qir(&bc, None).is_err(), "{declaration}");
        assert!(qir_to_qis(&bc, 0, "native", None).is_err(), "{declaration}");
    }
}

#[test]
#[allow(
    clippy::panic,
    clippy::unreachable,
    reason = "child process must fail if its runtime termination path returns"
)]
fn allocation_failure_without_error_pointer_terminates_after_rollback() {
    const ENV: &str = "QIR_QIS_RESULT_PANIC_CASE";
    if let Ok(case) = std::env::var(ENV) {
        let (body, dynamic, fail_after) = match case.as_str() {
            "scalar" => (
                "entry:\n %r = call ptr @__quantum__rt__result_allocate(ptr null)\n ret i64 0",
                true,
                Some(0),
            ),
            "bulk" => (
                "entry:\n %a = alloca [2 x ptr]\n call void @__quantum__rt__result_array_allocate(i64 2, ptr %a, ptr null)\n ret i64 0",
                true,
                Some(1),
            ),
            "bounds" => (
                "entry:\n %a = alloca [1 x ptr]\n store ptr inttoptr (i64 2 to ptr), ptr %a\n call void @__quantum__rt__result_array_record_output(i64 1, ptr %a, ptr @tag)\n ret i64 0",
                false,
                None,
            ),
            _ => unreachable!("known child case"),
        };
        execute(&qir(body, "", dynamic, true), 0, fail_after, 0);
        panic!("runtime error path returned");
    }
    for case in ["scalar", "bulk", "bounds"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "result_runtime::tests::allocation_failure_without_error_pointer_terminates_after_rollback", "--nocapture"])
            .env(ENV, case).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(77),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("qir-qis runtime panic: 1001, live slots: 0"));
        let expected_diagnostic = if case == "bounds" {
            "EXIT:INT:Static result index out of bounds."
        } else {
            "EXIT:INT:No memory available for result allocation."
        };
        assert!(
            stderr.contains(&format!("diagnostic: {expected_diagnostic}\n")),
            "{stderr}"
        );
    }
}

#[test]
fn static_ids_loaded_from_arrays_work_in_helpers() {
    let ll = qir(
        r"entry:
  %a = alloca [1 x ptr]
  store ptr null, ptr %a
  %id = load ptr, ptr %a
  call void @measure_static(ptr inttoptr (i64 1 to ptr), ptr %id)
  %read = call i1 @__quantum__rt__read_result(ptr %id)
  call void @__quantum__rt__result_array_record_output(i64 1, ptr %a, ptr @tag)
  ret i64 0",
        r"define void @measure_static(ptr %q, ptr %r) {
entry:
  call void @__quantum__qis__mz__body(ptr %q, ptr %r)
  ret void
}",
        false,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 0);
        assert_eq!(rt.outputs.first().expect("one output").1, vec![true]);
        assert_eq!((rt.reads, rt.decrements), (1, 1));
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(32))]
    #[test]
    fn static_array_ids_preserve_arbitrary_order_and_repetition(ids in proptest::collection::vec(0_u64..2, 0..12)) {
        let mut body = format!("entry:\n %a = alloca [{} x ptr]\n", ids.len());
        for (i, id) in ids.iter().enumerate() {
            write!(body, " %p{i} = getelementptr [{} x ptr], ptr %a, i64 0, i64 {i}\n store ptr inttoptr (i64 {id} to ptr), ptr %p{i}\n", ids.len()).unwrap();
        }
        write!(body, " call void @__quantum__qis__mz__body(ptr null, ptr null)\n call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 1 to ptr))\n call void @__quantum__rt__result_array_record_output(i64 {}, ptr %a, ptr @tag)\n ret i64 0", ids.len()).unwrap();
        let rt = execute(&qir(&body, "", false, true), 0, None, 0);
        proptest::prop_assert_eq!(&rt.outputs.first().expect("one output").1, &ids.iter().map(|id| *id == 1).collect::<Vec<_>>());
        proptest::prop_assert_eq!(rt.decrements, 2);
    }
}

#[test]
fn bulk_helper_results_survive_local_backing_array_return() {
    let ll = qir(
        r"entry:
  %a = alloca [2 x ptr]
  %second = getelementptr [2 x ptr], ptr %a, i64 0, i64 1
  call void @allocate_pair(ptr %a, ptr %second)
  %r0 = load ptr, ptr %a
  %r1 = load ptr, ptr %second
  call void @__quantum__qis__mz__body(ptr null, ptr %r0)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr %r1)
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %a, ptr @tag)
  call void @__quantum__rt__result_array_release(i64 2, ptr %a)
  ret i64 0",
        r"define void @allocate_pair(ptr %out0, ptr %out1) {
entry:
  %local = alloca [2 x ptr]
  call void @__quantum__rt__result_array_allocate(i64 2, ptr %local, ptr null)
  %second = getelementptr [2 x ptr], ptr %local, i64 0, i64 1
  %r0 = load ptr, ptr %local
  %r1 = load ptr, ptr %second
  store ptr %r0, ptr %out0
  store ptr %r1, ptr %out1
  ret void
}",
        true,
        true,
    );
    for level in [0, 2] {
        let rt = execute(&ll, level, None, 0);
        assert_eq!(
            rt.outputs.first().expect("array output").1,
            vec![false, true]
        );
        assert_eq!((rt.allocated, rt.freed), (2, 2));
    }
}

#[test]
fn conditional_allocation_runs_only_on_taken_branch() {
    for (qubit, allocations) in [(0, 1), (1, 2)] {
        let ll = qir(
            &format!(
                r"entry:
  %control = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 {qubit} to ptr), ptr %control)
  %take = call i1 @__quantum__rt__read_result(ptr %control)
  call void @__quantum__rt__result_release(ptr %control)
  br i1 %take, label %taken, label %done
taken:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__rt__result_release(ptr %r)
  br label %done
done:
  ret i64 0"
            ),
            "",
            true,
            false,
        );
        for level in [0, 2] {
            let rt = execute(&ll, level, None, 0);
            assert_eq!((rt.allocated, rt.freed), (allocations, allocations));
        }
    }
}

#[test]
fn result_panic_messages_have_exact_length_prefixes() {
    for (body, dynamic, global_name, payload) in [
        (
            "entry:\n %r = call ptr @__quantum__rt__result_allocate(ptr null)\n call void @__quantum__rt__result_release(ptr %r)\n ret i64 0",
            true,
            "qir_qis.result_allocation_error",
            "EXIT:INT:No memory available for result allocation.",
        ),
        (
            "entry:\n %a = alloca [1 x ptr]\n store ptr null, ptr %a\n call void @__quantum__rt__result_array_record_output(i64 1, ptr %a, ptr @tag)\n ret i64 0",
            false,
            "qir_qis.static_result_bounds_error",
            "EXIT:INT:Static result index out of bounds.",
        ),
    ] {
        let bc = qir_ll_to_bc(&qir(body, "", dynamic, true)).unwrap();
        let compiled = qir_to_qis(&bc, 0, "native", None).unwrap();
        let ctx = Context::create();
        let buffer = MemoryBuffer::create_from_memory_range_copy(&compiled, "panic_messages");
        let module = Module::parse_bitcode_from_buffer(&buffer, &ctx).unwrap();
        let global = module.get_global(global_name).expect("panic message");
        let encoded = crate::convert::get_string_label(global).unwrap();
        let (length, bytes) = encoded.as_bytes().split_first().unwrap();
        assert_eq!(usize::from(*length), payload.len());
        assert_eq!(bytes, payload.as_bytes());
        assert_eq!(
            u32::try_from(encoded.len()).unwrap(),
            global.get_value_type().into_array_type().len()
        );
    }
}

#[test]
fn null_tag_scalar_output_accepts_loaded_and_selected_static_ids() {
    let ll = qir(
        r"entry:
  %a = alloca [1 x ptr]
  store ptr inttoptr (i64 1 to ptr), ptr %a
  call void @__quantum__qis__mz__body(ptr null, ptr null)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 1 to ptr))
  %id = load ptr, ptr %a
  %selected = select i1 true, ptr %id, ptr null
  call void @__quantum__rt__result_record_output(ptr %id, ptr null)
  call void @__quantum__rt__result_record_output(ptr %selected, ptr null)
  call void @__quantum__rt__result_record_output(ptr null, ptr null)
  call void @__quantum__rt__result_array_record_output(i64 1, ptr %a, ptr @tag)
  ret i64 0",
        "",
        false,
        true,
    );
    for level in [0, 2] {
        let expected = vec![
            ("USER:RESULT:result_static".into(), vec![true]),
            ("USER:RESULT:result_static".into(), vec![true]),
            ("USER:RESULT:result_0".into(), vec![false]),
            ("USER:RESULT_ARRAY:out".into(), vec![true]),
        ];
        for _ in 0..2 {
            assert_eq!(execute(&ll, level, None, 0).outputs, expected);
        }
    }
}

#[test]
fn static_helper_measurements_do_not_require_array_output() {
    for arrays in [false, true] {
        for operation in ["mz", "mresetz"] {
            let helpers = format!(
                r"define void @measure_static(ptr %q, ptr %r) {{
entry:
  call void @__quantum__qis__{operation}__body(ptr %q, ptr %r)
  ret void
}}
declare void @__quantum__qis__mresetz__body(ptr, ptr) #1"
            );
            let ll = qir(
                r"entry:
  call void @measure_static(ptr inttoptr (i64 1 to ptr), ptr null)
  %read = call i1 @__quantum__rt__read_result(ptr null)
  call void @__quantum__rt__result_record_output(ptr null, ptr @tag)
  ret i64 0",
                &helpers,
                false,
                arrays,
            );
            for level in [0, 2] {
                let rt = execute(&ll, level, None, 0);
                assert_eq!(rt.outputs, vec![("USER:RESULT:out".into(), vec![true])]);
                assert_eq!((rt.reads, rt.decrements, rt.allocated), (1, 1, 0));
            }
        }
    }
}

#[test]
fn helpers_can_record_scalar_and_array_results() {
    for dynamic in [false, true] {
        for record_array in [false, true] {
            let allocate = if dynamic {
                "%r = call ptr @__quantum__rt__result_allocate(ptr null)"
            } else {
                "%r = inttoptr i64 0 to ptr"
            };
            let release = if dynamic {
                "call void @__quantum__rt__result_release(ptr %r)"
            } else {
                ""
            };
            let array_output = if record_array {
                r"%a = alloca [2 x ptr]
  %second = getelementptr [2 x ptr], ptr %a, i64 0, i64 1
  store ptr %r, ptr %a
  store ptr %r, ptr %second
  call void @__quantum__rt__result_array_record_output(i64 2, ptr %a, ptr @tag)"
            } else {
                ""
            };
            let helpers = format!(
                r"define void @record() {{
entry:
  {allocate}
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr %r)
  call void @__quantum__rt__result_record_output(ptr %r, ptr @tag)
  {array_output}
  {release}
  ret void
}}"
            );
            let ll = qir(
                "entry:\n call void @record()\n ret i64 0",
                &helpers,
                dynamic,
                record_array,
            );
            for level in [0, 2] {
                let rt = execute(&ll, level, None, 0);
                let mut expected = vec![("USER:RESULT:out".into(), vec![true])];
                if record_array {
                    expected.push(("USER:RESULT_ARRAY:out".into(), vec![true, true]));
                }
                assert_eq!(rt.outputs, expected);
                assert_eq!((rt.reads, rt.decrements), (1, 1));
                assert_eq!(
                    (rt.allocated, rt.freed),
                    (usize::from(dynamic), usize::from(dynamic))
                );
            }
        }
    }
}

#[test]
fn input_native_calls_in_helpers_remain_rejected_without_validation() {
    for (declaration, call) in [
        ("declare void @___reset(i64)", "call void @___reset(i64 0)"),
        (
            "declare i64 @___lazy_measure(i64)",
            "%native = call i64 @___lazy_measure(i64 0)",
        ),
        (
            "declare void @print_bool(ptr, i64, i1)",
            "call void @print_bool(ptr null, i64 0, i1 false)",
        ),
    ] {
        for before in [false, true] {
            let (prefix, suffix) = if before { (call, "") } else { ("", call) };
            let helpers = format!(
                r"{declaration}
define void @record() {{
entry:
  {prefix}
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @__quantum__qis__mz__body(ptr null, ptr %r)
  {suffix}
  call void @__quantum__rt__result_release(ptr %r)
  ret void
}}"
            );
            let ll = qir(
                "entry:\n call void @record()\n ret i64 0",
                &helpers,
                true,
                false,
            );
            let bc = qir_ll_to_bc(&ll).unwrap();
            let error = qir_to_qis(&bc, 0, "native", None)
                .expect_err("input native call must not inherit generated-call permission");
            assert!(
                error.contains("Unexpected call to internal function"),
                "{error}"
            );
        }
    }
}

#[test]
fn helper_tag_parameters_are_rejected_without_looking_up_their_names() {
    for parameter in ["a", "missing"] {
        for array in [false, true] {
            let output = if array {
                format!(
                    "%results = alloca [1 x ptr]\n store ptr null, ptr %results\n call void @__quantum__rt__result_array_record_output(i64 1, ptr %results, ptr %{parameter})"
                )
            } else {
                format!(
                    "call void @__quantum__rt__result_record_output(ptr null, ptr %{parameter})"
                )
            };
            let helpers = format!(
                r#"
@a = private constant [2 x i8] c"A\00"
@b = private constant [2 x i8] c"B\00"
define void @record(ptr %{parameter}) {{
  {output}
  ret void
}}
"#
            );
            let ll = qir(
                "entry:\n call void @record(ptr @b)\n ret i64 0",
                &helpers,
                false,
                array,
            );
            let bc = qir_ll_to_bc(&ll).unwrap();
            for error in [
                validate_qir(&bc, None),
                qir_to_qis(&bc, 0, "native", None).map(|_| ()),
            ] {
                assert!(
                    error
                        .expect_err("runtime tags require specialization, not name guessing")
                        .contains("constant global string")
                );
            }
        }
    }
}

#[test]
fn helper_global_tag_views_keep_the_actual_global_label() {
    let ll = qir(
        r"entry:
  call void @record()
  ret i64 0",
        r"define void @record() {
  %a = getelementptr [4 x i8], ptr @tag, i64 0, i64 0
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr null)
  call void @__quantum__rt__result_record_output(ptr null, ptr %a)
  ret void
}",
        false,
        false,
    );
    for level in [0, 2] {
        assert_eq!(
            execute(&ll, level, None, 0).outputs,
            vec![("USER:RESULT:out".into(), vec![true])]
        );
    }
}

#[test]
fn computed_qubit_pointers_use_the_selected_qubit() {
    for phi in [false, true] {
        let select = if phi {
            r"br i1 %take, label %yes, label %no
yes:
  br label %merge
no:
  br label %merge
merge:
  %q = phi ptr [inttoptr (i64 1 to ptr), %yes], [null, %no]"
        } else {
            "%q = select i1 %take, ptr inttoptr (i64 1 to ptr), ptr null"
        };
        let helpers = format!(
            r"define void @measure_selected(i1 %take, ptr %r) {{
entry:
  {select}
  call void @__quantum__qis__mz__body(ptr %q, ptr %r)
  ret void
}}"
        );
        let ll = qir(
            r"entry:
  %r = call ptr @__quantum__rt__result_allocate(ptr null)
  call void @measure_selected(i1 false, ptr %r)
  call void @__quantum__rt__result_record_output(ptr %r, ptr @tag)
  call void @measure_selected(i1 true, ptr %r)
  call void @__quantum__rt__result_record_output(ptr %r, ptr @tag)
  call void @__quantum__rt__result_release(ptr %r)
  ret i64 0",
            &helpers,
            true,
            false,
        );
        for level in [0, 2] {
            assert_eq!(
                execute(&ll, level, None, 0).outputs,
                vec![
                    ("USER:RESULT:out".into(), vec![false]),
                    ("USER:RESULT:out".into(), vec![true]),
                ]
            );
        }
    }
}

#[test]
fn computed_static_ids_use_runtime_fallback_labels() {
    for phi in [false, true] {
        let select = if phi {
            r"br i1 %take, label %yes, label %no
yes:
  br label %merge
no:
  br label %merge
merge:
  %r = phi ptr [inttoptr (i64 1 to ptr), %yes], [null, %no]"
        } else {
            "%r = select i1 %take, ptr inttoptr (i64 1 to ptr), ptr null"
        };
        let helpers = format!(
            r"define void @record_selected(i1 %take) {{
entry:
  {select}
  call void @__quantum__rt__result_record_output(ptr %r, ptr null)
  ret void
}}"
        );
        let ll = qir(
            r"entry:
  call void @__quantum__qis__mz__body(ptr null, ptr null)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 1 to ptr))
  call void @record_selected(i1 false)
  call void @record_selected(i1 true)
  ret i64 0",
            &helpers,
            false,
            false,
        );
        for level in [0, 2] {
            assert_eq!(
                execute(&ll, level, None, 0).outputs,
                vec![
                    ("USER:RESULT:result_static".into(), vec![false]),
                    ("USER:RESULT:result_static".into(), vec![true]),
                ]
            );
        }
    }
}

#[test]
fn reserved_aliases_cannot_hide_result_storage_or_runtime_symbols() {
    for symbol in [
        "qir_qis.static_results",
        "qir_qis.result_read",
        "heap_alloc",
        "heap_free",
        "panic",
        "___read_future_bool",
        "___dec_future_refcount",
        "___lazy_measure",
        "print_bool",
        "print_bool_arr",
    ] {
        let helper = format!("@storage = private global i8 0\n @{symbol} = alias i8, ptr @storage");
        let ll = qir(
            "entry:\n %a = alloca [0 x ptr]\n call void @__quantum__rt__result_array_record_output(i64 0, ptr %a, ptr @tag)\n ret i64 0",
            &helper,
            false,
            true,
        );
        let bc = qir_ll_to_bc(&ll).unwrap();
        for error in [
            validate_qir(&bc, None),
            qir_to_qis(&bc, 0, "native", None).map(|_| ()),
        ] {
            assert!(
                error
                    .expect_err("aliases must not bypass reserved symbols")
                    .contains("reserved symbol")
            );
        }
    }
    let ll = qir(
        "entry:\n ret i64 0",
        r"@qir_qis.result_read = ifunc i1 (ptr), ptr @resolver
define ptr @resolver() { ret ptr @actual }
define i1 @actual(ptr %r) { ret i1 false }",
        true,
        false,
    );
    let bc = qir_ll_to_bc(&ll).unwrap();
    assert!(
        qir_to_qis(&bc, 0, "native", None)
            .expect_err("indirect functions also reserve names")
            .contains("reserved symbol")
    );
}

#[test]
fn result_signatures_are_checked_independently_of_static_bounds() {
    for name in [
        "__quantum__rt__read_result",
        "__quantum__rt__result_record_output",
        "__quantum__qis__mz__body",
        "__quantum__qis__m__body",
        "__quantum__qis__mresetz__body",
    ] {
        let read = name == "__quantum__rt__read_result";
        let canonical = if read {
            format!("declare i1 @{name}(ptr)")
        } else {
            format!("declare void @{name}(ptr, ptr)")
        };
        let signatures = if read {
            vec![
                "void (ptr)",
                "i32 (ptr)",
                "ptr (ptr)",
                "i1 ()",
                "i1 (ptr, ptr)",
                "i1 (i64)",
                "i1 (i32)",
                "i1 (ptr, ...)",
            ]
        } else {
            vec![
                "i1 (ptr, ptr)",
                "ptr (ptr, ptr)",
                "void ()",
                "void (ptr)",
                "void (ptr, ptr, ptr)",
                "void (i64, ptr)",
                "void (ptr, i64)",
                "void (ptr, ptr, ...)",
            ]
        };
        for signature in signatures {
            let (ret, args) = signature.split_once(' ').unwrap();
            let malformed = format!("declare {ret} @{name}{args}");
            for dynamic in [false, true] {
                let mut ll = qir("entry:\n ret i64 0", "", dynamic, false);
                if ll.contains(&canonical) {
                    ll = ll.replace(&canonical, &malformed);
                } else {
                    write!(ll, "\n{malformed}\n").unwrap();
                }
                let bc = qir_ll_to_bc(&ll).unwrap();
                for error in [
                    validate_qir(&bc, None),
                    qir_to_qis(&bc, 0, "native", None).map(|_| ()),
                ] {
                    let error = error
                        .expect_err("malformed declarations must return errors before lowering");
                    assert!(
                        error.contains("Malformed QIR result function declaration"),
                        "{malformed}: {error}"
                    );
                }
            }
        }
    }
    let ll = qir(
        "entry:\n %v = call i1 @__quantum__rt__read_result(i64 0)\n ret i64 0",
        "",
        true,
        false,
    )
    .replace(
        "declare i1 @__quantum__rt__read_result(ptr)",
        "declare i1 @__quantum__rt__read_result(i64)",
    );
    let bc = qir_ll_to_bc(&ll).unwrap();
    assert!(validate_qir(&bc, None).is_err());
    qir_to_qis(&bc, 0, "native", None)
        .expect_err("malformed result calls must fail without panicking");
}

#[test]
fn static_table_initialization_uses_memset_at_resource_limit() {
    for count in [1, crate::MAX_STATIC_RESULTS] {
        let ll = qir("entry:\n %a = alloca [0 x ptr]\n call void @__quantum__rt__result_array_record_output(i64 0, ptr %a, ptr @tag)\n ret i64 0", "", false, true)
            .replace("\"required_num_results\"=\"2\"", &format!("\"required_num_results\"=\"{count}\""));
        let bc = qir_ll_to_bc(&ll).unwrap();
        validate_qir(&bc, None).unwrap();
        let output = qir_to_qis(&bc, 0, "native", None).unwrap();
        let ctx = Context::create();
        let buffer = MemoryBuffer::create_from_memory_range_copy(&output, "bounded_initialization");
        let module = Module::parse_bitcode_from_buffer(&buffer, &ctx).unwrap();
        module.verify().unwrap();
        let initialize = module.get_function("qir_qis.init_static_results").unwrap();
        let calls: Vec<_> = initialize
            .get_basic_blocks()
            .into_iter()
            .flat_map(inkwell::basic_block::BasicBlock::get_instructions)
            .filter_map(|i| inkwell::values::CallSiteValue::try_from(i).ok())
            .filter_map(inkwell::values::CallSiteValue::get_called_fn_value)
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls.first().unwrap().get_name().to_str().unwrap(),
            "llvm.memset.p0.i64"
        );
    }
}

#[test]
fn entry_computed_static_ids_require_table_backing() {
    for table in [false, true] {
        let array_output = if table {
            "%a = alloca [0 x ptr]\n call void @__quantum__rt__result_array_record_output(i64 0, ptr %a, ptr @tag)"
        } else {
            ""
        };
        let ll = qir(
            &format!(
                r"entry:
  call void @__quantum__qis__mz__body(ptr null, ptr null)
  call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr inttoptr (i64 1 to ptr))
  %take = call i1 @__quantum__rt__read_result(ptr null)
  %id = select i1 %take, ptr inttoptr (i64 1 to ptr), ptr null
  call void @__quantum__rt__result_record_output(ptr %id, ptr null)
  {array_output}
  ret i64 0"
            ),
            "",
            false,
            table,
        );
        if table {
            for level in [0, 2] {
                assert_eq!(
                    execute(&ll, level, None, 0).outputs,
                    vec![
                        ("USER:RESULT:result_static".into(), vec![false]),
                        ("USER:RESULT_ARRAY:out".into(), vec![]),
                    ]
                );
            }
        } else {
            let bc = qir_ll_to_bc(&ll).unwrap();
            for error in [
                validate_qir(&bc, None),
                qir_to_qis(&bc, 0, "native", None).map(|_| ()),
            ] {
                assert!(
                    error
                        .expect_err("entry-only SSA lowering rejects computed IDs")
                        .contains("Cannot extract pointer index")
                );
            }
        }
    }
}

#[test]
fn user_memset_intrinsics_are_not_reported_as_internal_runtime_calls() {
    let ll = qir(
        "entry:\n call void @initialize()\n ret i64 0",
        r"define void @initialize() {
  %buffer = alloca [8 x i8]
  call void @llvm.memset.p0.i64(ptr %buffer, i8 0, i64 8, i1 false)
  ret void
}

declare void @llvm.memset.p0.i64(ptr, i8, i64, i1)",
        false,
        false,
    );
    let bc = qir_ll_to_bc(&ll).unwrap();
    let error = qir_to_qis(&bc, 0, "native", None).expect_err("user intrinsic remains unsupported");
    assert_eq!(
        error,
        "Unsupported LLVM intrinsic in user-defined function: llvm.memset.p0.i64"
    );
}

#[test]
fn cyclic_output_tag_views_fail_validation_and_direct_translation() {
    for pointers in [
        "%p = getelementptr i8, ptr %p, i64 0",
        "%p = getelementptr i8, ptr %q, i64 0\n %q = getelementptr i8, ptr %p, i64 0",
        "%p = bitcast ptr %q to ptr\n %q = getelementptr i8, ptr %p, i64 0",
        "%p = addrspacecast ptr addrspace(1) %q to ptr\n %q = addrspacecast ptr %p to ptr addrspace(1)",
    ] {
        for array in [false, true] {
            let output = if array {
                "%a = alloca [0 x ptr]\n call void @__quantum__rt__result_array_record_output(i64 0, ptr %a, ptr %p)"
            } else {
                "call void @__quantum__rt__result_record_output(ptr null, ptr %p)"
            };
            let body = format!("entry:\n ret i64 0\ndead:\n {pointers}\n {output}\n unreachable");
            let bc = qir_ll_to_bc(&qir(&body, "", false, true)).unwrap();
            for error in [
                validate_qir(&bc, None),
                qir_to_qis(&bc, 0, "native", None).map(|_| ()),
            ] {
                assert!(error.unwrap_err().contains("Cyclic result output tag"));
            }
        }
    }
}

#[test]
fn downstream_result_runtime_signatures_are_checked_before_lowering() {
    for (name, result, params, returned) in [
        ("___read_future_bool", "i1", "i64", "ret i1 false"),
        ("___dec_future_refcount", "void", "i64", "ret void"),
        ("___lazy_measure", "i64", "i64", "ret i64 0"),
        ("print_bool", "void", "ptr, i64, i1", "ret void"),
        ("print_bool_arr", "void", "ptr, i64, ptr", "ret void"),
    ] {
        let wrong_result = if result == "void" { "i64" } else { "void" };
        let wrong_kind = if params.contains("ptr") {
            params.replace("ptr", "i64")
        } else {
            params.replace("i64", "ptr")
        };
        let mut declarations = vec![
            format!("declare {wrong_result} @{name}({params})"),
            format!("declare {result} @{name}()"),
            format!("declare {result} @{name}({})", params.replace("i64", "i32")),
            format!("declare {result} @{name}({wrong_kind})"),
            format!("declare {result} @{name}({params}, ...)"),
            format!("define {result} @{name}({params}) {{ {returned} }}"),
            format!("@{name} = global i64 0"),
        ];
        if result != "void" {
            declarations.push(format!("declare i8 @{name}({params})"));
        }
        if params.contains("i1") {
            declarations.push(format!(
                "declare {result} @{name}({})",
                params.replace("i1", "i8")
            ));
        }
        for declaration in declarations {
            let bc = qir_ll_to_bc(&qir("entry:\n ret i64 0", &declaration, true, true)).unwrap();
            for error in [
                validate_qir(&bc, None),
                qir_to_qis(&bc, 0, "native", None).map(|_| ()),
            ] {
                let error = error.unwrap_err();
                assert!(
                    error.contains(&format!("Invalid downstream runtime declaration: {name}")),
                    "{error}"
                );
            }
        }
        let valid = format!("declare {result} @{name}({params})");
        let bc = qir_ll_to_bc(&qir("entry:\n ret i64 0", &valid, true, true)).unwrap();
        qir_to_qis(&bc, 0, "native", None).unwrap();
    }
}

#[test]
fn result_tag_labels_preserve_raw_bytes_and_enforce_abi_length() {
    for array in [false, true] {
        let limit = if array { 237 } else { 243 };
        for label in [
            String::new(),
            "part:bit".into(),
            "é".repeat(58),
            "x".repeat(115),
            "x".repeat(116),
            "x".repeat(limit),
            "x".repeat(limit + 1),
        ] {
            let output = if array {
                "%a = alloca [1 x ptr]\n store ptr null, ptr %a\n call void @__quantum__rt__result_array_record_output(i64 1, ptr %a, ptr @0)"
            } else {
                "call void @__quantum__rt__result_record_output(ptr null, ptr @0)"
            };
            let body = format!(
                "entry:\n call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr null)\n {output}\n ret i64 0"
            );
            let global = format!(
                "@0 = private constant [{} x i8] c\"{label}\\00\"",
                label.len() + 1
            );
            let ll = qir(&body, &global, false, array);
            if label.len() > limit {
                let bc = qir_ll_to_bc(&ll).unwrap();
                for error in [
                    validate_qir(&bc, None),
                    qir_to_qis(&bc, 0, "native", None).map(|_| ()),
                ] {
                    assert!(error.unwrap_err().contains("Constant string too long"));
                }
            } else {
                for level in [0, 2] {
                    let rt = execute(&ll, level, None, 0);
                    let kind = if array { "RESULT_ARRAY" } else { "RESULT" };
                    assert_eq!(
                        rt.outputs,
                        vec![(format!("USER:{kind}:{label}"), vec![true])]
                    );
                }
            }
        }
    }
}

#[test]
fn nonzero_and_runtime_result_tag_offsets_are_rejected() {
    for view in [
        "%p = getelementptr i8, ptr @tag, i64 1",
        "%i = select i1 true, i64 0, i64 1\n %p = getelementptr i8, ptr @tag, i64 %i",
    ] {
        for array in [false, true] {
            let output = if array {
                "%a = alloca [0 x ptr]\n call void @__quantum__rt__result_array_record_output(i64 0, ptr %a, ptr %p)"
            } else {
                "call void @__quantum__rt__result_record_output(ptr null, ptr %p)"
            };
            let ll = qir(
                &format!("entry:\n {view}\n {output}\n ret i64 0"),
                "",
                false,
                true,
            );
            let bc = qir_ll_to_bc(&ll).unwrap();
            for error in [
                validate_qir(&bc, None),
                qir_to_qis(&bc, 0, "native", None).map(|_| ()),
            ] {
                assert!(error.unwrap_err().contains("zero-index views"));
            }
        }
    }
}

#[test]
fn null_result_array_tags_are_rejected_before_lowering() {
    let ll = qir(
        "entry:\n %a = alloca [0 x ptr]\n call void @__quantum__rt__result_array_record_output(i64 0, ptr %a, ptr null)\n ret i64 0",
        "",
        false,
        true,
    );
    let bc = qir_ll_to_bc(&ll).unwrap();
    for error in [
        validate_qir(&bc, None),
        qir_to_qis(&bc, 0, "native", None).map(|_| ()),
    ] {
        assert!(
            error
                .unwrap_err()
                .contains("requires a constant global tag")
        );
    }
}

#[test]
fn unsupported_result_tag_globals_fail_before_translation() {
    for global in [
        "@label = constant [4 x i8] c\"out\\00\"",
        "@label = private global [4 x i8] c\"out\\00\"",
        "@label = private constant [1 x ptr] zeroinitializer",
    ] {
        let ll = qir(
            "entry:\n call void @__quantum__rt__result_record_output(ptr null, ptr @label)\n ret i64 0",
            global,
            false,
            false,
        );
        let bc = qir_ll_to_bc(&ll).unwrap();
        for error in [
            validate_qir(&bc, None),
            qir_to_qis(&bc, 0, "native", None).map(|_| ()),
        ] {
            assert!(error.unwrap_err().contains("constant global string"));
        }
    }
}

#[test]
fn fallback_result_labels_do_not_clobber_input_global_labels() {
    for (dynamic, computed, name) in [
        (true, false, "result_dynamic"),
        (false, false, "result_0"),
        (false, true, "result_static"),
    ] {
        let allocate = if dynamic {
            "%r = call ptr @__quantum__rt__result_allocate(ptr null)"
        } else if computed {
            "%r = select i1 true, ptr null, ptr inttoptr (i64 1 to ptr)"
        } else {
            ""
        };
        let result = if dynamic || computed { "%r" } else { "null" };
        let release = if dynamic {
            "call void @__quantum__rt__result_release(ptr %r)"
        } else {
            ""
        };
        let body = format!(
            "entry:\n {allocate}\n call void @__quantum__qis__mz__body(ptr inttoptr (i64 1 to ptr), ptr {result})\n call void @__quantum__rt__result_record_output(ptr {result}, ptr null)\n call void @__quantum__rt__result_record_output(ptr {result}, ptr @{name})\n %a = alloca [1 x ptr]\n store ptr {result}, ptr %a\n call void @__quantum__rt__result_array_record_output(i64 1, ptr %a, ptr @{name})\n {release}\n ret i64 0"
        );
        let global = format!("@{name} = private constant [7 x i8] c\"custom\\00\"");
        for level in [0, 2] {
            assert_eq!(
                execute(&qir(&body, &global, dynamic, true), level, None, 0).outputs,
                vec![
                    (format!("USER:RESULT:{name}"), vec![true]),
                    ("USER:RESULT:custom".into(), vec![true]),
                    ("USER:RESULT_ARRAY:custom".into(), vec![true]),
                ]
            );
        }
    }
}

#[test]
fn qir_result_functions_must_be_declarations() {
    for (signature, returned) in [
        ("ptr @__quantum__rt__result_allocate(ptr)", "ret ptr null"),
        ("void @__quantum__rt__result_release(ptr)", "ret void"),
        (
            "void @__quantum__rt__result_array_allocate(i64, ptr, ptr)",
            "ret void",
        ),
        (
            "void @__quantum__rt__result_array_release(i64, ptr)",
            "ret void",
        ),
        (
            "void @__quantum__rt__result_array_record_output(i64, ptr, ptr)",
            "ret void",
        ),
        (
            "void @__quantum__rt__result_record_output(ptr, ptr)",
            "ret void",
        ),
        ("i1 @__quantum__rt__read_result(ptr)", "ret i1 false"),
        ("void @__quantum__qis__mz__body(ptr, ptr)", "ret void"),
        ("void @__quantum__qis__m__body(ptr, ptr)", "ret void"),
        ("void @__quantum__qis__mresetz__body(ptr, ptr)", "ret void"),
    ] {
        let declaration = format!("declare {signature}");
        let ll = qir(
            "entry:\n ret i64 0",
            &format!("define {signature} {{ {returned} }}"),
            false,
            true,
        )
        .lines()
        .filter(|line| !line.starts_with(&declaration))
        .collect::<Vec<_>>()
        .join("\n");
        let bc = qir_ll_to_bc(&ll).unwrap();
        for error in [
            validate_qir(&bc, None),
            qir_to_qis(&bc, 0, "native", None).map(|_| ()),
        ] {
            assert!(
                error
                    .unwrap_err()
                    .contains("must be an external declaration")
            );
        }
    }
}

#[test]
fn helpers_sharing_entry_attributes_use_static_result_storage() {
    let ll = qir(
        "entry:\n call void @measure(ptr inttoptr (i64 1 to ptr), ptr null)\n call void @__quantum__rt__result_record_output(ptr null, ptr @tag)\n ret i64 0",
        "define void @measure(ptr %q, ptr %r) #0 {\n call void @__quantum__qis__mz__body(ptr %q, ptr %r)\n ret void\n}",
        false,
        false,
    );
    for level in [0, 2] {
        assert_eq!(
            execute(&ll, level, None, 0).outputs,
            vec![("USER:RESULT:out".into(), vec![true])]
        );
    }
}

#[test]
fn computed_static_results_require_nonzero_declared_storage() {
    let ll = qir("entry:\n call void @measure(ptr null, ptr null)\n ret i64 0", "define void @measure(ptr %q, ptr %r) {\n call void @__quantum__qis__mz__body(ptr %q, ptr %r)\n ret void\n}", false, false).replace("\"required_num_results\"=\"2\"", "\"required_num_results\"=\"0\"");
    let bc = qir_ll_to_bc(&ll).unwrap();
    for error in [
        validate_qir(&bc, None),
        qir_to_qis(&bc, 0, "native", None).map(|_| ()),
    ] {
        assert!(
            error
                .unwrap_err()
                .contains("require at least one static result")
        );
    }
    let empty = qir(
        "entry:\n %a = alloca [0 x ptr]\n call void @__quantum__rt__result_array_record_output(i64 0, ptr %a, ptr @tag)\n ret i64 0",
        "",
        false,
        true,
    )
    .replace("\"required_num_results\"=\"2\"", "\"required_num_results\"=\"0\"");
    for level in [0, 2] {
        assert_eq!(
            execute(&empty, level, None, 0).outputs,
            vec![("USER:RESULT_ARRAY:out".into(), vec![])]
        );
    }
}

#[test]
fn helper_result_operations_cannot_bypass_capability_checks() {
    let ll = qir(
        "entry:\n call void @allocate()\n ret i64 0",
        "define void @allocate() {\n %r = call ptr @__quantum__rt__result_allocate(ptr null)\n ret void\n}",
        false,
        false,
    );
    let bc = qir_ll_to_bc(&ll).unwrap();
    for error in [
        validate_qir(&bc, None),
        qir_to_qis(&bc, 0, "native", None).map(|_| ()),
    ] {
        assert!(
            error
                .unwrap_err()
                .contains("requires `dynamic_result_management=true`")
        );
    }
}
