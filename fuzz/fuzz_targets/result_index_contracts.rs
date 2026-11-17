#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};

fn pick_u8_inclusive(data: &[u8], cursor: &mut usize, min: u8, max: u8) -> u8 {
    let span = max.saturating_sub(min).saturating_add(1);
    let value = data.get(*cursor).copied().unwrap_or_default() % span;
    *cursor = cursor.saturating_add(1);
    min.saturating_add(value)
}

fn pick<'a>(data: &[u8], cursor: &mut usize, options: &'a [&'a str]) -> &'a str {
    let idx = usize::from(data.get(*cursor).copied().unwrap_or_default()) % options.len();
    *cursor = cursor.saturating_add(1);
    options[idx]
}

fn build_result_index_fixture(data: &[u8]) -> String {
    let mut cursor = 0;
    let required_num_qubits = pick_u8_inclusive(data, &mut cursor, 1, 2);
    let required_num_results = pick_u8_inclusive(data, &mut cursor, 0, 4);
    let result_index = pick_u8_inclusive(data, &mut cursor, 0, 7);
    let qir_major = pick(data, &mut cursor, &["1", "2"]);
    let mode = pick(
        data,
        &mut cursor,
        &[
            "measure_only",
            "measure_read",
            "measure_record",
            "measure_read_record",
            "cyclic_record",
            "cyclic_backing_allocate",
            "cyclic_backing_release",
            "cyclic_backing_output",
            "mismatched_read",
            "mismatched_allocate",
        ],
    );

    let mut declarations = vec!["declare void @__quantum__qis__mz__body(%Qubit*, %Result*)"];
    let mut body = vec![
        "  %q0 = inttoptr i64 1 to %Qubit*".to_string(),
        format!("  %r = inttoptr i64 {result_index} to %Result*"),
        "  call void @__quantum__qis__mz__body(%Qubit* %q0, %Result* %r)".to_string(),
    ];

    if mode.contains("read") {
        declarations.push("declare i1 @__quantum__rt__read_result(%Result*)");
        body.push("  %b = call i1 @__quantum__rt__read_result(%Result* %r)".to_string());
    }

    if mode.contains("record") {
        declarations.push("declare void @__quantum__rt__result_record_output(%Result*, ptr)");
        body.push(
            "  call void @__quantum__rt__result_record_output(%Result* %r, ptr @result_label)"
                .to_string(),
        );
    }

    if mode == "mismatched_read" {
        let argument = pick(
            data,
            &mut cursor,
            &["i64 0", "i32 0", "ptr null, ptr null", ""],
        );
        body.push(format!("  call i1 @__quantum__rt__read_result({argument})"));
    }
    if mode == "mismatched_allocate" {
        declarations.push("declare ptr @__quantum__rt__result_allocate(ptr)");
        let argument = pick(
            data,
            &mut cursor,
            &["i64 0", "i32 0", "ptr null, ptr null", ""],
        );
        body.push(format!(
            "  call ptr @__quantum__rt__result_allocate({argument})"
        ));
    }

    let result_label = if mode.contains("record") {
        "@result_label = private constant [2 x i8] c\"r\\00\"\n\n"
    } else {
        ""
    };

    // LLVM permits cyclic SSA in unreachable blocks; tag resolution must stop.
    let mut dead = String::new();
    if mode == "cyclic_record" {
        let count = pick_u8_inclusive(data, &mut cursor, 1, 8);
        dead.push_str("dead:\n");
        for index in 0..count {
            let next = (index + 1) % count;
            dead.push_str(&format!(
                "  %tag{index} = getelementptr i8, ptr %tag{next}, i64 0\n"
            ));
        }
        dead.push_str("  call void @__quantum__rt__result_record_output(%Result* null, ptr %tag0)\n  unreachable\n");
    }

    let dynamic = mode.starts_with("cyclic_backing");
    if dynamic {
        let count = pick_u8_inclusive(data, &mut cursor, 1, 8);
        dead.push_str("dead:\n");
        for index in 0..count {
            let next = (index + 1) % count;
            dead.push_str(&format!(
                "  %buffer{index} = getelementptr i8, ptr %buffer{next}, i64 0\n"
            ));
        }
        let (declaration, call) = match mode {
            "cyclic_backing_allocate" => (
                "declare void @__quantum__rt__result_array_allocate(i64, ptr, ptr)",
                "call void @__quantum__rt__result_array_allocate(i64 0, ptr %buffer0, ptr null)",
            ),
            "cyclic_backing_release" => (
                "declare void @__quantum__rt__result_array_release(i64, ptr)",
                "call void @__quantum__rt__result_array_release(i64 0, ptr %buffer0)",
            ),
            _ => (
                "declare void @__quantum__rt__result_array_record_output(i64, ptr, ptr)",
                "call void @__quantum__rt__result_array_record_output(i64 0, ptr %buffer0, ptr null)",
            ),
        };
        declarations.push(declaration);
        dead.push_str(&format!("  {call}\n  unreachable\n"));
    }

    format!(
        r#"%Qubit = type opaque
%Result = type opaque

{result_label}{}

define i64 @Entry_Point_Name() #0 {{
entry:
{}
  ret i64 0
{dead}
}}

attributes #0 = {{ "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="{required_num_qubits}" "required_num_results"="{required_num_results}" }}

!llvm.module.flags = !{{!0, !1, !2, !3, !4}}
!0 = !{{i32 1, !"qir_major_version", i32 {qir_major}}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
!2 = !{{i32 1, !"dynamic_qubit_management", i1 false}}
!3 = !{{i32 1, !"dynamic_result_management", i1 {dynamic}}}
!4 = !{{i32 1, !"arrays", i1 true}}
"#,
        declarations.join("\n"),
        body.join("\n")
    )
}

fuzz_target!(|data: &[u8]| -> Corpus {
    let ll_text = build_result_index_fixture(data);
    let Ok(bitcode) = qir_qis::qir_ll_to_bc(&ll_text) else {
        return Corpus::Reject;
    };

    let _ = qir_qis::validate_qir(&bitcode, None);
    let _ = qir_qis::qir_to_qis(&bitcode, 0, "native", None);
    Corpus::Keep
});
