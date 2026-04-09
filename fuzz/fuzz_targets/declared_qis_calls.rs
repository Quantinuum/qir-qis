#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};

const BARRIER_ARITY_LIMIT: u8 = 4;

fn pick<'a>(data: &[u8], cursor: &mut usize, options: &'a [&'a str]) -> &'a str {
    let idx = usize::from(data.get(*cursor).copied().unwrap_or_default()) % options.len();
    *cursor = cursor.saturating_add(1);
    options[idx]
}

fn pick_u8_inclusive(data: &[u8], cursor: &mut usize, min: u8, max: u8) -> u8 {
    let span = max.saturating_sub(min).saturating_add(1);
    let value = data.get(*cursor).copied().unwrap_or_default() % span;
    *cursor = cursor.saturating_add(1);
    min.saturating_add(value)
}

fn build_declared_qis_fixture(data: &[u8]) -> String {
    let mut cursor = 0;
    let kind = pick(
        data,
        &mut cursor,
        &[
            "single",
            "angle_single",
            "angle_double",
            "two_qubit",
            "measure",
            "reset",
            "barrier",
            "unsupported",
        ],
    );
    let suffix = pick(data, &mut cursor, &["__body", "__adj", "__ctl", ""]);

    let (declaration, call, required_num_qubits, required_num_results) = match kind {
        "single" => {
            let stem = pick(
                data,
                &mut cursor,
                &["h", "x", "y", "z", "s", "t", "cx", "cnot", "cz", "ccx"],
            );
            match stem {
                "cx" | "cnot" | "cz" => {
                    let name = format!("__quantum__qis__{stem}{suffix}");
                    (
                        format!("declare void @{name}(%Qubit*, %Qubit*)"),
                        format!("  call void @{name}(%Qubit* %q0, %Qubit* %q1)"),
                        2,
                        1,
                    )
                }
                "ccx" => {
                    let name = format!("__quantum__qis__{stem}{suffix}");
                    (
                        format!("declare void @{name}(%Qubit*, %Qubit*, %Qubit*)"),
                        format!("  call void @{name}(%Qubit* %q0, %Qubit* %q1, %Qubit* %q2)"),
                        3,
                        1,
                    )
                }
                _ => {
                    let name = format!("__quantum__qis__{stem}{suffix}");
                    (
                        format!("declare void @{name}(%Qubit*)"),
                        format!("  call void @{name}(%Qubit* %q0)"),
                        1,
                        1,
                    )
                }
            }
        }
        "angle_single" => {
            let stem = pick(data, &mut cursor, &["rx", "ry", "rz"]);
            let name = format!("__quantum__qis__{stem}{suffix}");
            (
                format!("declare void @{name}(double, %Qubit*)"),
                format!("  call void @{name}(double 1.0, %Qubit* %q0)"),
                1,
                1,
            )
        }
        "angle_double" => {
            let stem = pick(data, &mut cursor, &["rxy", "rzz", "u1q"]);
            let name = format!("__quantum__qis__{stem}{suffix}");
            let signature = if stem == "u1q" {
                "double, double, %Qubit*"
            } else if stem == "rzz" {
                "double, %Qubit*, %Qubit*"
            } else {
                "double, double, %Qubit*"
            };
            let call = if stem == "rzz" {
                format!("  call void @{name}(double 1.0, %Qubit* %q0, %Qubit* %q1)")
            } else {
                format!("  call void @{name}(double 1.0, double 0.5, %Qubit* %q0)")
            };
            (
                format!("declare void @{name}({signature})"),
                call,
                if stem == "rzz" { 2 } else { 1 },
                1,
            )
        }
        "measure" => {
            let stem = pick(data, &mut cursor, &["m", "mz", "mresetz"]);
            let name = format!("__quantum__qis__{stem}{suffix}");
            (
                format!("declare void @{name}(%Qubit*, %Result*)"),
                format!("  call void @{name}(%Qubit* %q0, %Result* %r0)"),
                1,
                1,
            )
        }
        "reset" => {
            let name = format!("__quantum__qis__reset{suffix}");
            (
                format!("declare void @{name}(%Qubit*)"),
                format!("  call void @{name}(%Qubit* %q0)"),
                1,
                1,
            )
        }
        "barrier" => {
            let arity = pick_u8_inclusive(data, &mut cursor, 1, BARRIER_ARITY_LIMIT);
            let name = format!("__quantum__qis__barrier{arity}{suffix}");
            let decl_args = std::iter::repeat_n("%Qubit*", usize::from(arity))
                .collect::<Vec<_>>()
                .join(", ");
            let call_args = (0..arity)
                .map(|idx| format!("%Qubit* %q{idx}"))
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!("declare void @{name}({decl_args})"),
                format!("  call void @{name}({call_args})"),
                arity,
                1,
            )
        }
        _ => {
            let stem = pick(data, &mut cursor, &["mystery", "teleport", "foo"]);
            let name = format!("__quantum__qis__{stem}{suffix}");
            (
                format!("declare void @{name}(%Qubit*)"),
                format!("  call void @{name}(%Qubit* %q0)"),
                1,
                1,
            )
        }
    };

    let qubit_setup = (0..required_num_qubits)
        .map(|idx| {
            let one_based = idx.saturating_add(1);
            format!("  %q{idx} = inttoptr i64 {one_based} to %Qubit*")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let result_setup = if required_num_results > 0 {
        "  %r0 = inttoptr i64 1 to %Result*".to_string()
    } else {
        String::new()
    };

    format!(
        r#"%Qubit = type opaque
%Result = type opaque

{declaration}

define i64 @Entry_Point_Name() #0 {{
entry:
{qubit_setup}
{result_setup}
{call}
  ret i64 0
}}

attributes #0 = {{ "entry_point" "qir_profiles"="base_profile" "output_labeling_schema"="schema_id" "required_num_qubits"="{required_num_qubits}" "required_num_results"="{required_num_results}" }}

!llvm.module.flags = !{{!0, !1, !2, !3}}
!0 = !{{i32 1, !"qir_major_version", i32 1}}
!1 = !{{i32 7, !"qir_minor_version", i32 0}}
!2 = !{{i32 1, !"dynamic_qubit_management", i1 false}}
!3 = !{{i32 1, !"dynamic_result_management", i1 false}}
"#
    )
}

fuzz_target!(|data: &[u8]| -> Corpus {
    let ll_text = build_declared_qis_fixture(data);
    let Ok(bitcode) = qir_qis::qir_ll_to_bc(&ll_text) else {
        return Corpus::Reject;
    };

    if qir_qis::validate_qir(&bitcode, None).is_ok() {
        let _ = qir_qis::qir_to_qis(&bitcode, 0, "native", None);
    }
    Corpus::Keep
});
