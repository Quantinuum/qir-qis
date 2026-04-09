#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};

fn pick<'a>(data: &[u8], cursor: &mut usize, options: &'a [&'a str]) -> &'a str {
    let idx = usize::from(data.get(*cursor).copied().unwrap_or_default()) % options.len();
    *cursor = cursor.saturating_add(1);
    options[idx]
}

fn build_entry_contract_fixture(data: &[u8]) -> String {
    let mut cursor = 0;
    let mut attrs = vec![
        "\"entry_point\"".to_string(),
        format!(
            "\"qir_profiles\"=\"{}\"",
            pick(data, &mut cursor, &["base_profile", "adaptive_profile", ""])
        ),
        format!(
            "\"output_labeling_schema\"=\"{}\"",
            pick(data, &mut cursor, &["schema_id", "labeled", ""])
        ),
        format!(
            "\"required_num_qubits\"=\"{}\"",
            pick(data, &mut cursor, &["0", "1", "2", "8"])
        ),
        format!(
            "\"required_num_results\"=\"{}\"",
            pick(data, &mut cursor, &["0", "1", "2", "8"])
        ),
    ];
    if data.get(cursor).copied().unwrap_or_default() % 2 == 1 {
        attrs.push(format!(
            "\"custom_attr\"=\"{}\"",
            pick(data, &mut cursor, &["alpha", "beta", "gamma", ""])
        ));
    }
    let rotation = usize::from(data.get(cursor).copied().unwrap_or_default()) % attrs.len();
    attrs.rotate_left(rotation);
    cursor = cursor.saturating_add(1);

    let qir_major_first = pick(data, &mut cursor, &["1", "2", "99"]);
    let qir_major_second = pick(data, &mut cursor, &["1", "2", "100"]);
    let qir_minor = pick(data, &mut cursor, &["0", "1"]);
    let dynamic_qubit = pick(data, &mut cursor, &["false", "true"]);
    let dynamic_result = pick(data, &mut cursor, &["false", "true"]);
    let include_duplicate_major = data.get(cursor).copied().unwrap_or_default() % 2 == 1;

    let mut flags = vec![
        format!("!0 = !{{i32 1, !\"qir_major_version\", i32 {qir_major_first}}}"),
        format!("!1 = !{{i32 7, !\"qir_minor_version\", i32 {qir_minor}}}"),
        format!("!2 = !{{i32 1, !\"dynamic_qubit_management\", i1 {dynamic_qubit}}}"),
        format!("!3 = !{{i32 1, !\"dynamic_result_management\", i1 {dynamic_result}}}"),
    ];
    let flag_refs = if include_duplicate_major {
        flags.push(format!(
            "!4 = !{{i32 1, !\"qir_major_version\", i32 {qir_major_second}}}"
        ));
        "!llvm.module.flags = !{!0, !1, !2, !3, !4}"
    } else {
        "!llvm.module.flags = !{!0, !1, !2, !3}"
    };

    format!(
        r#"define i64 @Entry_Point_Name() #0 {{
entry:
  ret i64 0
}}

attributes #0 = {{ {} }}

{flag_refs}
{}
"#,
        attrs.join(" "),
        flags.join("\n")
    )
}

fuzz_target!(|data: &[u8]| -> Corpus {
    let ll_text = build_entry_contract_fixture(data);
    let Ok(bitcode) = qir_qis::qir_ll_to_bc(&ll_text) else {
        return Corpus::Reject;
    };

    let _ = qir_qis::get_entry_attributes(&bitcode);
    let _ = qir_qis::validate_qir(&bitcode, None);
    Corpus::Keep
});
